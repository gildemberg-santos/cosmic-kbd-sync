use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use ashpd::desktop::settings::Settings;
use ashpd::desktop::Color;
use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;
use tracing::{error, info, warn};

const DEFAULT_KBD_PATH: &str = "/sys/class/leds/asus::kbd_backlight/kbd_rgb_mode";
const APPEARANCE_NAMESPACE: &str = "org.freedesktop.appearance";
const ACCENT_KEY: &str = "accent-color";
const RECONNECT_DELAY: Duration = Duration::from_secs(2);
const RESUME_DELAY: Duration = Duration::from_secs(1);
const POLL_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rgb {
    red: u8,
    green: u8,
    blue: u8,
}

impl Rgb {
    fn from_color(color: Color) -> Self {
        Self {
            red: channel_to_u8(color.red()),
            green: channel_to_u8(color.green()),
            blue: channel_to_u8(color.blue()),
        }
    }

    fn payload(self) -> String {
        // ABI do asus-nb-wmi: save mode red green blue speed
        // save=1, mode=0 (estático), speed=0
        format!("1 0 {} {} {} 0\n", self.red, self.green, self.blue)
    }
}

fn channel_to_u8(value: f64) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[derive(Debug, Clone, Copy)]
enum Event {
    AccentChanged,
    Resumed,
}

struct Config {
    keyboard_path: PathBuf,
    dry_run: bool,
    once: bool,
}

impl Config {
    fn parse() -> Result<Self> {
        let mut keyboard_path = std::env::var_os("COSMIC_KBD_RGB_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_KBD_PATH));
        let mut dry_run = std::env::var_os("COSMIC_KBD_RGB_DRY_RUN")
            .map(|value| value == "1" || value == "true")
            .unwrap_or(false);
        let mut once = false;

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--once" => once = true,
                "--dry-run" => dry_run = true,
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                "--path" => {
                    keyboard_path = PathBuf::from(args.next().context("--path exige um valor")?);
                }
                other if other.starts_with("--path=") => {
                    keyboard_path = PathBuf::from(&other["--path=".len()..]);
                }
                other => bail!("argumento desconhecido: {other} (use --help)"),
            }
        }

        Ok(Self {
            keyboard_path,
            dry_run,
            once,
        })
    }
}

fn print_help() {
    println!("cosmic-kbd-sync [--once] [--dry-run] [--path PATH]");
    println!();
    println!("  --once       lê o accent atual, aplica uma vez e sai");
    println!("  --dry-run    lê o accent, mas não escreve no sysfs");
    println!("  --path PATH  sobrescreve o caminho do kbd_rgb_mode");
    println!();
    println!("Variáveis de ambiente:");
    println!("  COSMIC_KBD_RGB_PATH      caminho alternativo do kbd_rgb_mode");
    println!("  COSMIC_KBD_RGB_DRY_RUN=1 não escreve no sysfs");
    println!("  RUST_LOG                 nível de log (ex.: debug)");
}

struct Keyboard {
    path: PathBuf,
    dry_run: bool,
}

impl Keyboard {
    fn new(path: PathBuf, dry_run: bool) -> Self {
        Self { path, dry_run }
    }

    async fn apply(&self, rgb: Rgb) -> Result<bool> {
        let payload = rgb.payload();

        if self.dry_run {
            info!(
                path = %self.path.display(),
                ?rgb,
                payload = %payload.trim(),
                "dry-run: não escrevendo no sysfs"
            );
            return Ok(false);
        }

        let mut file = match tokio::fs::OpenOptions::new()
            .write(true)
            .open(&self.path)
            .await
        {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                bail!(
                    "sem permissão para escrever em {}; instale a regra udev (veja install.sh/README.md)",
                    self.path.display()
                );
            }
            Err(err) => {
                return Err(err).with_context(|| format!("falha ao abrir {}", self.path.display()));
            }
        };

        file.write_all(payload.as_bytes())
            .await
            .with_context(|| format!("falha ao escrever em {}", self.path.display()))?;
        file.flush()
            .await
            .with_context(|| format!("falha ao finalizar escrita em {}", self.path.display()))?;
        drop(file);
        Ok(true)
    }
}

struct Engine {
    keyboard: Keyboard,
    last: Option<Rgb>,
}

impl Engine {
    fn new(keyboard: Keyboard) -> Self {
        Self {
            keyboard,
            last: None,
        }
    }

    async fn handle(&mut self, event: Event) -> Result<()> {
        // Lê pelo ReadOne do portal, que devolve o accent corretamente como (ddd).
        // O sinal SettingChanged do COSMIC hoje manda um array "ad"; por isso não
        // usamos o valor do sinal, apenas o usamos como gatilho.
        let settings = Settings::new()
            .await
            .context("não foi possível conectar ao portal XDG")?;
        let color = settings
            .accent_color()
            .await
            .context("não foi possível ler org.freedesktop.appearance/accent-color")?;

        let rgb = Rgb::from_color(color);
        let force = matches!(event, Event::Resumed);

        if !force && self.last == Some(rgb) {
            return Ok(());
        }

        let written = self.keyboard.apply(rgb).await?;
        self.last = Some(rgb);
        if written {
            info!(?rgb, "accent aplicado ao backlight do teclado");
        } else {
            info!(?rgb, "accent lido (dry-run); nenhuma escrita feita");
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let config = Config::parse()?;
    let mut engine = Engine::new(Keyboard::new(config.keyboard_path, config.dry_run));

    if config.once {
        engine.handle(Event::AccentChanged).await?;
        return Ok(());
    }

    let (tx, mut rx) = mpsc::channel(16);
    tokio::spawn(settings_watcher(tx.clone()));
    tokio::spawn(resume_watcher(tx));

    while let Some(event) = rx.recv().await {
        if let Err(err) = engine.handle(event).await {
            error!("falha ao aplicar a cor: {err:#}");
        }
    }

    Ok(())
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("cosmic_kbd_sync=info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

async fn settings_watcher(tx: mpsc::Sender<Event>) -> ! {
    loop {
        if let Err(err) = settings_loop(&tx).await {
            warn!("watcher do portal falhou: {err:#}; reconectando");
        }
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

async fn settings_loop(tx: &mpsc::Sender<Event>) -> Result<()> {
    let settings = Settings::new().await.context("portal XDG indisponível")?;

    // Aplica a cor atual assim que o portal estiver disponível.
    tx.send(Event::AccentChanged)
        .await
        .context("canal de eventos fechado")?;

    let mut changes = settings
        .receive_setting_changed()
        .await
        .context("não foi possível assinar SettingChanged")?;

    // Fallback: mesmo que o sinal falhe ou seja perdido, relê o accent
    // periodicamente. O Engine ignora escritas repetidas.
    let mut poll = tokio::time::interval(POLL_INTERVAL);
    poll.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            setting = changes.next() => {
                let Some(setting) = setting else {
                    bail!("stream de SettingChanged terminou");
                };

                if setting.namespace() == APPEARANCE_NAMESPACE
                    && setting.key() == ACCENT_KEY
                {
                    info!("mudança de accent detectada no portal");
                    tx.send(Event::AccentChanged)
                        .await
                        .context("canal de eventos fechado")?;
                }
            }
            _ = poll.tick() => {
                tx.send(Event::AccentChanged)
                    .await
                    .context("canal de eventos fechado")?;
            }
        }
    }
}

async fn resume_watcher(tx: mpsc::Sender<Event>) -> ! {
    loop {
        if let Err(err) = resume_loop(&tx).await {
            warn!("watcher do logind falhou: {err:#}; reconectando");
        }
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

async fn resume_loop(tx: &mpsc::Sender<Event>) -> Result<()> {
    let connection = zbus::Connection::system()
        .await
        .context("system bus indisponível")?;
    let proxy = Login1ManagerProxy::new(&connection)
        .await
        .context("logind indisponível")?;
    let mut signals = proxy
        .receive_prepare_for_sleep()
        .await
        .context("não foi possível assinar PrepareForSleep")?;

    while let Some(signal) = signals.next().await {
        let args = signal.args().context("sinal PrepareForSleep inválido")?;
        if !args.start {
            info!("sistema acordou; reaplicando a cor");
            // Dá um instante para o EC/backlight voltar após o resume.
            tokio::time::sleep(RESUME_DELAY).await;
            tx.send(Event::Resumed)
                .await
                .context("canal de eventos fechado")?;
        }
    }

    bail!("stream de PrepareForSleep terminou")
}

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Login1Manager {
    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_cosmic_purple() {
        let color = Color::new(0.905_882_4, 0.611_764_7, 0.996_078_5);
        assert_eq!(
            Rgb::from_color(color),
            Rgb {
                red: 231,
                green: 156,
                blue: 254,
            }
        );
    }

    #[test]
    fn payload_uses_static_saved_mode() {
        assert_eq!(
            Rgb {
                red: 231,
                green: 156,
                blue: 254,
            }
            .payload(),
            "1 0 231 156 254 0\n"
        );
    }

    #[test]
    fn clamps_out_of_range_channels() {
        assert_eq!(channel_to_u8(-1.0), 0);
        assert_eq!(channel_to_u8(2.0), 255);
    }

    #[tokio::test]
    async fn keyboard_writes_payload() {
        let path = std::env::temp_dir().join(format!(
            "cosmic-kbd-sync-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        std::fs::write(&path, b"").unwrap();
        let keyboard = Keyboard::new(path.clone(), false);
        let written = keyboard
            .apply(Rgb {
                red: 1,
                green: 2,
                blue: 3,
            })
            .await
            .unwrap();

        assert!(written);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "1 0 1 2 3 0\n");
        std::fs::remove_file(path).unwrap();
    }
}
