# cosmic-kbd-sync

Sincroniza a cor de destaque (accent) do COSMIC no Pop!_OS com o backlight RGB
do teclado ASUS TUF. O serviço roda como usuário normal, escuta o portal XDG e
escreve a cor no atributo `kbd_rgb_mode` do driver `asus-nb-wmi`.

## Como funciona

1. O portal `org.freedesktop.portal.Settings` expõe
   `org.freedesktop.appearance` / `accent-color`.
2. O serviço assina `SettingChanged` e, quando o accent muda, relê o valor pelo
   método `Read` do portal.
3. A cor sRGB de 8 bits é convertida para o formato aceito pelo teclado:
   `save mode R G B speed`.
4. O serviço escreve `1 0 R G B 0` em
   `/sys/class/leds/asus::kbd_backlight/kbd_rgb_mode`
   (`save=1`, `mode=0` estático, `speed=0`).
5. Ao acordar da suspensão, o logind avisa o serviço, que reaplica a cor.

O accent atual `#E79CFE` vira:

```text
1 0 231 156 254 0
```

O teclado aceita RGB de 24 bits, então não é necessário escolher a cor
"mais próxima": a cor exata é enviada.

### Portal do COSMIC

O sinal `SettingChanged` do portal do COSMIC atualmente entrega o accent como
um array (`ad`) em vez da tupla `(ddd)` esperada pela API. Por isso o serviço
usa o sinal apenas como gatilho e relê o valor com `Settings::accent_color()`.
Também existe um polling de segurança a cada 10 segundos; escritas repetidas
são ignoradas.

## Requisitos

- Pop!_OS com COSMIC e `xdg-desktop-portal-cosmic`.
- Teclado ASUS TUF cujo driver exponha
  `/sys/class/leds/asus::kbd_backlight/kbd_rgb_mode` (modelos de 1 zona RGB).
- Rust/Cargo para compilar.
- `sudo` apenas para instalar a regra udev.

## Instalação

```bash
cd ~/cosmic-kbd-sync
chmod +x install.sh
./install.sh
```

O script:

- compila em modo release;
- instala o binário em `~/.local/bin/cosmic-kbd-sync`;
- instala a unidade de usuário em
  `~/.config/systemd/user/cosmic-kbd-sync.service`;
- instala `/etc/udev/rules.d/99-asus-kbd-rgb.rules`, que dá ao seu usuário
  permissão de escrita no `kbd_rgb_mode`;
- recarrega o udev;
- aplica a cor atual uma vez para testar a escrita no hardware;
- ativa e reinicia o serviço.

Se o teste de escrita falhar, o script para antes de ativar o serviço. O `sudo`
só é usado para a regra udev. O serviço em si **não** roda como root.

A regra udev é específica para o usuário que executa o script. Se o nome de
usuário mudar, rode o `install.sh` novamente.

### Instalação manual da regra udev

Se preferir fazer só essa parte manualmente:

```bash
sed "s/@USER@/$(id -un)/g" data/99-asus-kbd-rgb.rules | \
  sudo tee /etc/udev/rules.d/99-asus-kbd-rgb.rules >/dev/null
sudo udevadm control --reload-rules
sudo udevadm trigger --action=add --subsystem-match=leds
sudo udevadm settle
ls -l /sys/class/leds/asus::kbd_backlight/kbd_rgb_mode
```

O arquivo deve ficar com o seu usuário como dono e modo `0600`.

## Testes

Testes unitários (não precisam de hardware nem root):

```bash
cargo test
```

Leitura real do accent pelo portal, sem escrever no hardware:

```bash
cargo run -- --once --dry-run
```

Saída esperada com o accent atual:

```text
... dry-run: não escrevendo no sysfs ... rgb: Rgb { red: 231, green: 156, blue: 254 }
```

Aplicar uma vez de verdade (exige a regra udev instalada):

```bash
~/.local/bin/cosmic-kbd-sync --once
```

Teste manual direto no hardware, sem o serviço:

```bash
echo "1 0 231 156 254 0" | sudo tee /sys/class/leds/asus::kbd_backlight/kbd_rgb_mode
```

Se esse comando não mudar a cor, o problema está no firmware/EC ou no suporte
do modelo, não no serviço.

Acompanhar o serviço:

```bash
systemctl --user status cosmic-kbd-sync.service
journalctl --user -u cosmic-kbd-sync -f
```

Teste de mudança de accent:

1. Abra Configurações do COSMIC e troque a cor de destaque.
2. O journal deve mostrar `mudança de accent detectada no portal` e
   `accent aplicado ao backlight do teclado`.
3. O teclado deve acompanhar a cor.

Teste de suspensão:

1. Suspenda e volte.
2. O journal deve mostrar `sistema acordou; reaplicando a cor`.

## Limitações

- O alvo é o backlight do teclado, não as keycaps.
- Modelos de 1 zona mostram uma única cor no teclado inteiro.
- O serviço não altera o brilho (`brightness`, 0–3); só a cor.
- Se `kbd_rgb_mode` não existir, o modelo/firmware não expõe essa interface.
- O atributo é write-only no sysfs; não é possível ler a cor de volta.
- O polling de 10 segundos é uma rede de segurança, não o mecanismo principal.

## Desinstalação

```bash
systemctl --user disable --now cosmic-kbd-sync.service
rm -f ~/.config/systemd/user/cosmic-kbd-sync.service
rm -f ~/.local/bin/cosmic-kbd-sync
sudo rm -f /etc/udev/rules.d/99-asus-kbd-rgb.rules
sudo udevadm control --reload-rules
systemctl --user daemon-reload
```

## Estrutura

```text
Cargo.toml
src/main.rs
data/cosmic-kbd-sync.service
data/99-asus-kbd-rgb.rules
install.sh
```
