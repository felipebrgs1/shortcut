# shortcut — launcher para Linux (Tauri + Solid 2 + Tailwind)

Launcher estilo Spotlight: busca de aplicativos, calculadora e histórico de
área de transferência (texto, imagens e arquivos).

- **Frontend**: Solid 2 + Tailwind 4 + Vite
- **Backend**: Rust (Tauri 2) — os módulos de busca/clipboard/apps

## Desenvolvimento

```sh
npm install
npm run tauri dev
```

## Build de release

```sh
npm run build:release
# pacote: src-tauri/target/release/bundle/deb/shortcut_*.deb
# binário direto: src-tauri/target/release/shortcut
```

## Uso e atalho global (Alt+Space)

O app é de instância única: **rodar o binário de novo alterna a janela** (abre
se fechada, fecha se aberta).

O atalho **Alt+Space** é registrado automaticamente (X11); no **Wayland**, o
registro global via Tauri não funciona de forma confiável — configure um
atalho personalizado do KDE:

1. Preferências do Sistema → Atalhos → Atalhos personalizados → Novo →
   Atalho Global → Comando/URL
2. Trigger: `Alt+Space` · Ação: caminho do binário (ex: `shortcut` se instalado
   com o .deb, ou `src-tauri/target/release/shortcut`)

O atalho executa o binário, que alterna a janela pela instância única.

## Configurações (dentro do launcher)

Digite **"config"** (ou "atalho", "histórico"…) na busca para abrir as
configurações:

- **Máx. itens salvos** no histórico
- **Salvar imagens copiadas**
- **Limpar histórico**
- Arquivos copiados (ex: no Dolphin) são detectados automaticamente (uri-list)

O histórico fica em `~/.local/share/shortcut/clipboard.json` e as imagens em
`~/.local/share/shortcut/images/`.

## Notas

- **Colar** (depois de escolher um item do clipboard) exige `wtype`
  (`sudo pacman -S wtype`) no Wayland — sem ele, o item é apenas copiado.
- O histórico de imagens requer Klipper/`wl-copy` ou o protocolo data-control
  (o arboard faz o fallback automático).
- O AppImage não é gerado em alguns ambientes (linuxdeploy); use o `.deb`.
