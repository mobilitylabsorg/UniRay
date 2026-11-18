import { createRouter, createWebHashHistory } from "vue-router";

import { useAppStore } from "../stores/app";
import ConnectView from "../views/ConnectView.vue";

export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/", name: "connect", component: ConnectView },
    {
      path: "/servers",
      name: "servers",
      component: () => import("../views/ServersView.vue"),
      // A tab of its own on a TV only; elsewhere the list is on the connection screen.
      beforeEnter: () => (useAppStore().isTv ? true : { name: "connect", hash: "#servers" }),
    },
    { path: "/routing", name: "routing", component: () => import("../views/RoutingView.vue") },
    { path: "/logs", name: "logs", component: () => import("../views/LogsView.vue") },
    { path: "/settings", name: "settings", component: () => import("../views/SettingsView.vue") },
  ],
});
