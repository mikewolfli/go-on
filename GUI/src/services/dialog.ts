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

type DialogOpen = (options?: OpenDialogOptions) => Promise<string | string[] | null>;

async function loadTauriDialogOpen(): Promise<DialogOpen | null> {
    try {
        const pluginId = "@tauri-apps/plugin-dialog";
        const mod = (await import(/* @vite-ignore */ pluginId)) as {
            open?: DialogOpen;
        };
        if (typeof mod.open === "function") {
            return mod.open;
        }
        return null;
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
        return tauriOpen(options);
    }
    return webPromptFallback(options);
}
