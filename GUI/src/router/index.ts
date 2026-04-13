import { createRouter, createWebHashHistory } from "vue-router";

const routes = [
    { path: "/", redirect: "/dashboard" },
    { path: "/dashboard", component: () => import("../views/DashboardView.vue") },
    { path: "/monitor", component: () => import("../views/MonitorView.vue") },
    { path: "/setup", component: () => import("../views/SetupView.vue") },
    { path: "/config", component: () => import("../views/ConfigView.vue") },
    { path: "/providers", component: () => import("../views/ProvidersView.vue") },
    { path: "/backend-ops", component: () => import("../views/BackendOpsView.vue") },
    { path: "/logs", component: () => import("../views/LogsView.vue") },
    { path: "/ai-usage", component: () => import("../views/AiUsageView.vue") },
    { path: "/health-breakdown", component: () => import("../views/HealthBreakdownView.vue") },
    { path: "/autotune", component: () => import("../views/AutoTuneView.vue") },
    { path: "/workflow", component: () => import("../views/WorkflowView.vue") },
    { path: "/security", component: () => import("../views/SecurityView.vue") },
    { path: "/mini", component: () => import("../views/MiniConsoleView.vue") },
];

export default createRouter({
    history: createWebHashHistory(),
    routes,
});
