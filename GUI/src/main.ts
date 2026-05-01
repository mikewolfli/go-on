import { createApp } from "vue";
import { createPinia } from "pinia";
import { ElMessage } from "element-plus";
import App from "./App.vue";
import router from "./router";
import { i18n } from "./locales";
import "./styles/dark.css";

const app = createApp(App);

// Global error handler
app.config.errorHandler = (err, _instance, info) => {
  if (import.meta.env.DEV) {
    console.error("Unhandled Vue error:", err, info);
  }
  const message =
    i18n.global.t("error.unexpected") ||
    "An unexpected error occurred. Please check the console for details.";
  ElMessage.error(message);
};

app.use(createPinia());
app.use(router);
app.use(i18n);
app.mount("#app");
