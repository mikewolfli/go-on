import { createApp } from "vue";
import { createPinia } from "pinia";
import { ElMessage } from "element-plus";
import App from "./App.vue";
import router from "./router";
import { i18n } from "./locales";
import "./styles/dark.css";

const app = createApp(App);

// Global error handler
// NOTE: English is intentional here — this is a last-resort handler where i18n may not be initialized.
app.config.errorHandler = (err, instance, info) => {
  console.error("Unhandled Vue error:", err, info);
  ElMessage.error(
    "An unexpected error occurred. Please check the console for details.",
  );
};

app.use(createPinia());
app.use(router);
app.use(i18n);
app.mount("#app");
