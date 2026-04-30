import { createRouter, createWebHashHistory } from "vue-router";
import MiniConsoleView from "../views/MiniConsoleView.vue";

const routes = [
  { path: "/", component: { template: "<div></div>" } },
  { path: "/mini", component: MiniConsoleView },
];

export default createRouter({
  history: createWebHashHistory(),
  routes,
});
