#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
USER_NAME="$(id -un)"
USER_HOME="$(getent passwd "$USER_NAME" | cut -d: -f6)"

BIN_DIR="$USER_HOME/.local/bin"
UNIT_DIR="$USER_HOME/.config/systemd/user"
RULE_SRC="$PROJECT_DIR/data/99-asus-kbd-rgb.rules"
RULE_DST="/etc/udev/rules.d/99-asus-kbd-rgb.rules"
KBD_PATH="/sys/class/leds/asus::kbd_backlight/kbd_rgb_mode"

if [[ $EUID -eq 0 ]]; then
    echo "Rode este script como usuário normal; ele chama sudo apenas para a regra udev." >&2
    exit 1
fi

if [[ ! -e "$KBD_PATH" ]]; then
    echo "Aviso: $KBD_PATH não existe. O serviço vai compilar, mas não conseguirá aplicar a cor." >&2
fi

echo "==> Compilando"
cargo build --release --manifest-path "$PROJECT_DIR/Cargo.toml"

echo "==> Instalando binário em $BIN_DIR"
install -Dm755 "$PROJECT_DIR/target/release/cosmic-kbd-sync" "$BIN_DIR/cosmic-kbd-sync"

echo "==> Instalando unidade systemd em $UNIT_DIR"
install -Dm644 "$PROJECT_DIR/data/cosmic-kbd-sync.service" "$UNIT_DIR/cosmic-kbd-sync.service"

echo "==> Instalando regra udev em $RULE_DST (sudo)"
sed "s/@USER@/$USER_NAME/g" "$RULE_SRC" | sudo tee "$RULE_DST" >/dev/null
sudo udevadm control --reload-rules
sudo udevadm trigger --action=add --subsystem-match=leds
sudo udevadm settle

echo "==> Permissões atuais:"
ls -l "$KBD_PATH" || true

if [[ -e "$KBD_PATH" && ! -w "$KBD_PATH" ]]; then
    echo "ERRO: $KBD_PATH ainda não é gravável pelo usuário $USER_NAME." >&2
    echo "Confira a regra udev e tente novamente (ou reinicie a sessão)." >&2
    exit 1
fi

if [[ -e "$KBD_PATH" ]]; then
    echo "==> Aplicando a cor atual uma vez (teste de hardware)"
    "$BIN_DIR/cosmic-kbd-sync" --once
fi

echo "==> Ativando o serviço de usuário"
systemctl --user daemon-reload
systemctl --user enable cosmic-kbd-sync.service
systemctl --user restart cosmic-kbd-sync.service

echo "==> Status"
systemctl --user --no-pager status cosmic-kbd-sync.service || true

echo
echo "Pronto. Acompanhe com: journalctl --user -u cosmic-kbd-sync -f"
