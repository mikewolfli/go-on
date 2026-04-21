import { createRouter, createWebHashHistory } from "vue-router";
import MiniConsoleView from "../views/MiniConsoleView.vue";

const routes = [
    { path: "/", redirect: "/dashboard" },
    { path: "/dashboard", redirect: "/" },
    { path: "/monitor", redirect: "/" },
    { path: "/setup", redirect: "/" },
    { path: "/config", redirect: "/" },
    { path: "/providers", redirect: "/" },
    { path: "/backend-ops", redirect: "/" },
    { path: "/logs", redirect: "/" },
    { path: "/ai-usage", redirect: "/" },
    { path: "/health-breakdown", redirect: "/" },
    { path: "/autotune", redirect: "/" },
    { path: "/workflow", redirect: "/" },
    { path: "/security", redirect: "/" },
    { path: "/mini", component: MiniConsoleView },
];

export default createRouter({
    history: createWebHashHistory(),
    routes,
});
