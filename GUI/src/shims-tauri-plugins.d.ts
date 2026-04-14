// Type shim for @tauri-apps/plugin-dialog.
// Provides ambient declarations so vue-tsc resolves imports without
// requiring the package to be installed in the local node_modules.
// The actual implementation is provided by Tauri at runtime.
declare module '@tauri-apps/plugin-dialog' {
    export interface OpenDialogFilter {
        name: string;
        extensions: string[];
    }

    export interface OpenDialogOptions {
        multiple?: boolean;
        directory?: boolean;
        title?: string;
        filters?: OpenDialogFilter[];
        defaultPath?: string;
    }

    export interface SaveDialogOptions {
        title?: string;
        filters?: OpenDialogFilter[];
        defaultPath?: string;
    }

    export function open(options?: OpenDialogOptions): Promise<string | string[] | null>;
    export function save(options?: SaveDialogOptions): Promise<string | null>;
}
