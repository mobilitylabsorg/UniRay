import { createRouter, createWebHashHistory } from "vue-router";

import ConnectView from "../views/ConnectView.vue";

export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/", name: "connect", component: ConnectView },
    // The servers were a page of their own; they are the list on the connection screen now.
    { path: "/servers", redirect: { name: "connect", hash: "#servers" } },
    { path: "/routing", name: "routing", component: () => import("../views/RoutingView.vue") },
    { path: "/logs", name: "logs", component: () => import("../views/LogsView.vue") },
    { path: "/settings", name: "settings", component: () => import("../views/SettingsView.vue") },
  ],
});
