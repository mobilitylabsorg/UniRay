<script setup lang="ts">
import ServerList from "../components/ServerList.vue";
</script>

<template>
  <!-- A TV: the servers as a tab of their own. -->
  <div class="mx-auto w-full max-w-5xl">
    <ServerList standalone />
  </div>
</template>
