import { createRouter, createWebHashHistory } from "vue-router";

import HomeView from "../views/HomeView.vue";

export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/", name: "home", component: HomeView },
    { path: "/servers", name: "servers", component: () => import("../views/ServersView.vue") },
    { path: "/routing", name: "routing", component: () => import("../views/RoutingView.vue") },
    { path: "/logs", name: "logs", component: () => import("../views/LogsView.vue") },
    { path: "/settings", name: "settings", component: () => import("../views/SettingsView.vue") },
  ],
});
