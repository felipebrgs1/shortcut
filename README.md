# shortcut — launcher nativo para Linux (sem webview)

Launcher estilo Spotlight: busca de aplicativos, calculadora e histórico de
área de transferência. **100% Rust + egui/eframe** — sem Tauri, sem webview,
sem dependência de JavaScript/Node.

## Build

```sh
cargo build --release
# binário: target/release/shortcut (~19 MB)
```

Dependências de sistema: libs do winit (Wayland/X11) — normalmente já presentes
em qualquer desktop Linux. No Arch/KDE não é preciso instalar nada além de um
toolchain Rust.

## Uso

O app é de **instância única**: rodar o binário de novo alterna a janela. Ou
seja:

1. `target/release/shortcut` — mostra/esconde o launcher
2. Esc fecha · ↑↓ navega · Enter executa · clique fora esconde

### Atalho global no KDE

Preferências do Sistema → Atalhos → Atalhos personalizados → Novo → Atalho
Global → **Comando/URL**, e aponte para o caminho do binário. O atalho alterna
o launcher.

### Instalação opcional

```sh
cargo build --release
install -Dm755 target/release/shortcut ~/.local/bin/shortcut
install -Dm644 shortcut.desktop ~/.local/share/applications/shortcut.desktop
```

(`shortcut.desktop` usa `Exec=~/.local/bin/shortcut`.)

## Dependências em tempo de execução

| Função | Requisito | Obs |
|---|---|---|
| Colar (no app alvo) | `wtype` | Wayland não permite injetar teclas; instale com `sudo pacman -S wtype` |
| Definir clipboard | — | Klipper (qdbus) → `wl-copy` → arboard (data-control), nessa ordem |
| Ícones SVG/PNG de apps | — | embutido (resvg via egui_extras) |

## Detalhes de implementação

- **Janela**: 720×464, sem decoração, transparente (X11 e Wayland) com
  cantos realmente arredondados — o anti-aliasing funde com o desktop.
- **Foco**: esconde ao perder foco, mas só depois de ter recebido foco ao
  menos uma vez (evita sumir se o compositor negar ativação).
- **Debug**: `SHORTCUT_KEEP=1 shortcut` desativa o blur-hide (útil para
  inspecionar a janela).
- **Persistência do histórico**: `~/.local/share/shortcut/clipboard.json`
  (via `dirs::data_local_dir`).
- **Single-instance**: Unix socket em `$XDG_RUNTIME_DIR/shortcut.sock`.

## Estrutura

```
src/
├── main.rs       ← UI egui + janela + single-instance
├── apps.rs       ← varredura de .desktop + resolução de ícones
├── clipboard.rs  ← watcher de clipboard + histórico + colar
└── search.rs     ← busca fuzzy + calculadora
assets/icon.png   ← ícone da janela
shortcut.desktop  ← instalação no menu do desktop
```