import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import AutoImport from "unplugin-auto-import/vite";
import Components from "unplugin-vue-components/vite";
import { ElementPlusResolver } from "unplugin-vue-components/resolvers";

export default defineConfig({
    plugins: [
        vue(),
        AutoImport({
            resolvers: [ElementPlusResolver()],
        }),
        Components({
            resolvers: [ElementPlusResolver()],
        }),
    ],
    server: {
        port: 5174,
        strictPort: true,
    },
    build: {
        rollupOptions: {
            output: {
                manualChunks(id) {
                    if (id.includes("node_modules")) {
                        if (id.includes("element-plus")) {
                            return "ui-element-plus";
                        }
                        if (id.includes("echarts")) {
                            return "viz-echarts";
                        }
                        if (id.includes("@tauri-apps")) {
                            return "tauri-runtime";
                        }
                        if (id.includes("vue") || id.includes("pinia") || id.includes("vue-router")) {
                            return "vue-core";
                        }
                        return "vendor";
                    }
                    return undefined;
                },
            },
        },
    },
});
