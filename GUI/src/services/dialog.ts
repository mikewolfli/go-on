type OpenDialogFilter = {
  name: string;
  extensions: string[];
};

type OpenDialogOptions = {
  multiple?: boolean;
  directory?: boolean;
  title?: string;
  filters?: OpenDialogFilter[];
  defaultPath?: string;
};

type DialogOpen = (
  options?: OpenDialogOptions,
) => Promise<string | string[] | null>;

let tauriDialogOpen: DialogOpen | null | undefined = undefined;

async function loadTauriDialogOpen(): Promise<DialogOpen | null> {
  if (tauriDialogOpen !== undefined) return tauriDialogOpen;
  try {
    const pluginId = "@tauri-apps/plugin-dialog";
    const mod = (await import(/* @vite-ignore */ pluginId)) as {
      open?: DialogOpen;
    };
    tauriDialogOpen = mod?.open ?? null;
    return tauriDialogOpen;
  } catch {
    return null;
  }
}

function webPromptFallback(options?: OpenDialogOptions): string | null {
  if (typeof window === "undefined") {
    return null;
  }
  const promptText = options?.directory
    ? "Tauri dialog unavailable. Please input directory path manually:"
    : "Tauri dialog unavailable. Please input file path manually:";
  const value = window.prompt(promptText, options?.defaultPath ?? "");
  const normalized = value?.trim();
  return normalized ? normalized : null;
}

export async function openDialog(
  options?: OpenDialogOptions,
): Promise<string | string[] | null> {
  const tauriOpen = await loadTauriDialogOpen();
  if (tauriOpen) {
    try {
      return tauriOpen(options);
    } catch {
      // fall through to web fallback
    }
  }
  const fallbackResult = webPromptFallback(options);
  if (options?.multiple) {
    return fallbackResult ? [fallbackResult] : null;
  }
  return fallbackResult;
}
