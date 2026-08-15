import { createEffect, createSignal, For, onSettled, Show } from "solid-js";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface ResultItem {
  kind: string;
  title: string;
  subtitle: string;
  icon: string | null;
  score: number;
  data: string;
}

interface Config {
  can_paste: boolean;
  max_history: number;
  save_images: boolean;
}

interface HistoryStats {
  n: number;
  images: number;
}

const KIND_LABELS: Record<string, string> = {
  calc: "Calculadora",
  app: "Aplicativos",
  settings: "Configurações",
  text: "Área de transferência",
  image: "Imagens",
  file: "Arquivos",
};

export default function App() {
  const [query, setQuery] = createSignal("");
  const [results, setResults] = createSignal<ResultItem[]>([]);
  const [selected, setSelected] = createSignal(0);
  const [canPaste, setCanPaste] = createSignal(false);
  const [mode, setMode] = createSignal<"search" | "settings">("search");
  const [maxHistory, setMaxHistory] = createSignal(300);
  const [saveImages, setSaveImages] = createSignal(true);
  const [stats, setStats] = createSignal<HistoryStats | null>(null);
  let inputRef: HTMLInputElement | undefined;
  let listRef: HTMLDivElement | undefined;

  async function doSearch(q: string = query()) {
    try {
      const r = await invoke<ResultItem[]>("search_cmd", { query: q });
      setResults(r);
      setSelected(0);
    } catch (e) {
      console.error("search_cmd falhou:", e);
    }
  }

  async function refreshConfig() {
    const c = await invoke<Config>("get_config").catch(() => null);
    if (c) {
      setCanPaste(c.can_paste);
      setMaxHistory(c.max_history);
      setSaveImages(c.save_images);
    }
  }

  async function refreshStats() {
    const s = await invoke<HistoryStats>("history_stats").catch(() => null);
    if (s) setStats(s);
  }

  function actionLabel(item: ResultItem | undefined): string {
    if (!item) return "";
    switch (item.kind) {
      case "app":
        return "Abrir";
      case "calc":
        return "Copiar resultado";
      case "settings":
        return "Abrir configurações";
      default:
        return canPaste() ? "Colar" : "Copiar";
    }
  }

  async function execute(item: ResultItem) {
    if (item.kind === "settings") {
      setMode("settings");
      void refreshConfig();
      void refreshStats();
      inputRef?.blur();
      return;
    }
    await invoke("execute", { result: item });
  }

  async function updateConfig(nextMax?: number, nextImages?: boolean) {
    await invoke("set_config", {
      maxHistory: nextMax ?? maxHistory(),
      saveImages: nextImages ?? saveImages(),
    });
    void refreshConfig();
    void refreshStats();
    void doSearch();
  }

  async function clearHistory() {
    await invoke("clear_history");
    void refreshStats();
    void doSearch();
  }

  function onKeydown(e: KeyboardEvent) {
    if (mode() === "settings") {
      if (e.key === "Escape") {
        setMode("search");
        e.preventDefault();
        inputRef?.focus();
      }
      return;
    }
    const rs = results();
    if (e.key === "ArrowDown") {
      setSelected(Math.min(selected() + 1, Math.max(rs.length - 1, 0)));
      e.preventDefault();
    } else if (e.key === "ArrowUp") {
      setSelected(Math.max(selected() - 1, 0));
      e.preventDefault();
    } else if (e.key === "Enter") {
      const item = rs[selected()];
      if (item) void execute(item);
      e.preventDefault();
    } else if (e.key === "Escape") {
      void invoke("hide_window");
    }
  }

  onSettled(() => {
    void refreshConfig();
    window.addEventListener("keydown", onKeydown);
    const unlistenShown = listen("window-shown", () => {
      setQuery("");
      setMode("search");
      void doSearch();
      inputRef?.focus();
      inputRef?.select();
    });
    const unlistenClip = listen("clipboard-changed", () => {
      if (mode() === "search") void doSearch();
    });
    // Solid 2: o cleanup é o retorno do onSettled
    return () => {
      void unlistenShown.then((f) => f());
      void unlistenClip.then((f) => f());
      window.removeEventListener("keydown", onKeydown);
    };
  });

  createEffect(
    () => query(),
    (q) => {
      void doSearch(q);
    }
  );

  createEffect(
    () => selected(),
    (s) => {
      listRef?.querySelector(`[data-index="${s}"]`)?.scrollIntoView({ block: "nearest" });
    }
  );

  function showHeader(i: number): boolean {
    return i === 0 || results()[i].kind !== results()[i - 1].kind;
  }

  return (
    <div class="flex h-screen w-screen items-start justify-center p-0">
      <div
        class="flex h-full w-full flex-col overflow-hidden rounded-xl border border-panel-border bg-panel shadow-2xl shadow-black/60"
        onmousedown={(e) => e.stopPropagation()}
      >
        {/* Cabeçalho: busca */}
        <div class="flex shrink-0 items-center gap-3 border-b border-panel-border px-4 py-3">
          <SearchIcon />
          <input
            ref={inputRef}
            value={query()}
            oninput={(e) => setQuery((e.target as HTMLInputElement).value)}
            placeholder="Buscar apps, clipboard ou calcular..."
            class="w-full bg-transparent text-[15px] text-text-main outline-none placeholder-text-dim"
            autofocus
          />
        </div>

        {/* Lista */}
        <Show when={mode() === "search"}>
          <div ref={listRef} class="min-h-0 flex-1 overflow-y-auto px-2 py-2">
            <Show when={results().length === 0}>
              <div class="px-3 py-8 text-center text-sm text-text-dim">
                Nenhum resultado
              </div>
            </Show>
            <For each={results()}>
              {(item, i) => (
                <>
                  <Show when={showHeader(i())}>
                    <div class="px-3 pt-2 pb-1 text-[11px] font-semibold tracking-wide text-text-dim uppercase">
                      {KIND_LABELS[item.kind] ?? item.kind}
                    </div>
                  </Show>
                  <button
                    data-index={i()}
                    class={`flex w-full items-center gap-3 rounded-lg px-3 py-2 text-left ${
                      i() === selected() ? "bg-item-active" : "hover:bg-item-hover"
                    }`}
                    onmousemove={() => setSelected(i())}
                    onclick={() => void execute(item)}
                  >
                    <ItemIcon item={item} />
                    <span class="min-w-0 flex-1">
                      <span class="block truncate text-sm text-text-main">
                        {item.title}
                      </span>
                      <Show when={item.subtitle}>
                        <span class="block truncate text-xs text-text-dim">
                          {item.subtitle}
                        </span>
                      </Show>
                    </span>
                    <Show when={i() === selected()}>
                      <span class="shrink-0 text-xs text-text-dim">
                        {actionLabel(item)} ⏎
                      </span>
                    </Show>
                  </button>
                </>
              )}
            </For>
          </div>
          {/* Rodapé */}
          <div class="flex shrink-0 items-center justify-between border-t border-panel-border px-4 py-2 text-[11px] text-text-dim">
            <span>
              ↑↓ navegar · Enter{" "}
              {actionLabel(results()[selected()]).toLowerCase() || "executar"} ·
              Esc fechar
            </span>
            <span>
              {results().length}{" "}
              {results().length === 1 ? "resultado" : "resultados"}
            </span>
          </div>
        </Show>

        {/* Configurações */}
        <Show when={mode() === "settings"}>
          <div class="min-h-0 flex-1 overflow-y-auto px-6 py-4">
            <SectionTitle>Atalho global</SectionTitle>
            <p class="mt-1 text-xs text-text-dim">
              X11: Alt+Space é registrado pelo app (ajustável nas Configurações
              do KDE/atributos). Wayland: crie um atalho personalizado do KDE
              apontando para o binário — ele alterna o launcher.
            </p>

            <div class="mt-5">
              <SectionTitle>Histórico da área de transferência</SectionTitle>
              <div class="mt-2 flex items-center justify-between">
                <span class="text-sm text-text-main">Máx. itens salvos</span>
                <div class="flex items-center gap-2">
                  <StepBtn
                    label="−"
                    onClick={() =>
                      void updateConfig(Math.max(maxHistory() - 50, 10))
                    }
                  />
                  <span class="w-10 text-center text-sm text-text-main">
                    {maxHistory()}
                  </span>
                  <StepBtn
                    label="+"
                    onClick={() =>
                      void updateConfig(Math.min(maxHistory() + 50, 1000))
                    }
                  />
                </div>
              </div>
              <label class="mt-3 flex cursor-pointer items-center gap-2 text-sm text-text-main">
                <input
                  type="checkbox"
                  checked={saveImages()}
                  onchange={(e) =>
                    void updateConfig(undefined, (e.target as HTMLInputElement).checked)
                  }
                  class="accent-[#7aa8ff]"
                />
                Salvar imagens copiadas
              </label>
              <div class="mt-2 text-xs text-text-dim">
                Arquivos copiados (ex: no Dolphin) são detectados
                automaticamente como uri-list.
              </div>
              <div class="mt-3 flex items-center justify-between">
                <span class="text-xs text-text-dim">
                  Armazenado: {stats()?.n ?? "—"} itens ({stats()?.images ?? 0}{" "}
                  imagens)
                </span>
                <button
                  onclick={() => void clearHistory()}
                  class="rounded-md bg-accent/20 px-3 py-1.5 text-xs font-medium text-accent transition hover:bg-accent/30"
                >
                  Limpar histórico
                </button>
              </div>
            </div>

            <p class="mt-6 text-center text-[11px] text-text-dim">
              Esc voltar
            </p>
          </div>
        </Show>
      </div>
    </div>
  );
}

function SearchIcon() {
  return (
    <svg class="h-4 w-4 shrink-0 text-text-dim" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
      <circle cx="11" cy="11" r="7" />
      <path d="m21 21-4.3-4.3" />
    </svg>
  );
}

function SectionTitle(props: { children: string }) {
  return (
    <div class="text-[11px] font-semibold tracking-wide text-text-dim uppercase">
      {props.children}
    </div>
  );
}

function StepBtn(props: { label: string; onClick: () => void }) {
  return (
    <button
      onclick={props.onClick}
      class="flex h-7 w-7 items-center justify-center rounded-md border border-panel-border text-sm text-text-main transition hover:bg-item-active"
    >
      {props.label}
    </button>
  );
}

function ItemIcon(props: { item: ResultItem }) {
  const item = () => props.item;
  return (
    <Show
      when={item().icon && (item().kind === "app" || item().kind === "image")}
      fallback={
        <Show
          when={item().kind === "calc"}
          fallback={
            <Show
              when={item().kind === "settings"}
              fallback={<GlyphIcon kind={item().kind} />}
            >
              <SlidersIcon />
            </Show>
          }
        >
          <span class="flex h-6 w-6 shrink-0 items-center justify-center rounded bg-accent/20 text-xs font-bold text-accent">
            =
          </span>
        </Show>
      }
    >
      <img
        src={convertFileSrc(item().icon!)}
        alt=""
        class="h-6 w-6 shrink-0"
      />
    </Show>
  );
}

function GlyphIcon(props: { kind: string }) {
  return (
    <span class="flex h-6 w-6 shrink-0 items-center justify-center rounded bg-white/5 text-text-dim">
      {props.kind === "file" ? (
        <svg class="h-3.5 w-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <rect x="3" y="4" width="18" height="16" rx="2" />
          <path d="M3 9h18" />
          <path d="M7 14h6" />
        </svg>
      ) : (
        <svg class="h-3.5 w-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <rect x="8" y="2" width="8" height="4" rx="1" />
          <path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2" />
        </svg>
      )}
    </span>
  );
}

function SlidersIcon() {
  return (
    <span class="flex h-6 w-6 shrink-0 items-center justify-center rounded bg-white/5 text-text-dim">
      <svg class="h-3.5 w-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <path d="M4 8h10" />
        <path d="M18 8h2" />
        <circle cx="16" cy="8" r="2" />
        <path d="M4 16h2" />
        <path d="M10 16h10" />
        <circle cx="8" cy="16" r="2" />
      </svg>
    </span>
  );
}
