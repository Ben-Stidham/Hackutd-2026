<script setup lang="ts">
import { RouterView } from 'vue-router'
</script>

<template>
  <div class="app-shell">
    <header class="topbar">
      <a class="brand" href="/" aria-label="Fix My OS home">
        <span class="brand-mark" aria-hidden="true">FM</span>
        <span>Fix My OS</span>
      </a>
      <div class="topbar-status">
        <span class="status-dot" aria-hidden="true"></span>
        LOCAL SESSION
      </div>
    </header>
    <RouterView />
  </div>
</template>

<style>
.app-shell {
  min-height: 100vh;
}

.topbar {
  height: 68px;
  padding: 0 clamp(20px, 5vw, 72px);
  display: flex;
  align-items: center;
  justify-content: space-between;
  border-bottom: 1px solid #dce5df;
  background: rgba(250, 252, 249, 0.88);
}

.brand {
  display: inline-flex;
  align-items: center;
  gap: 11px;
  color: #172620;
  text-decoration: none;
  font-size: 15px;
  font-weight: 700;
}

.brand-mark {
  width: 32px;
  height: 32px;
  display: grid;
  place-items: center;
  background: #d7ed74;
  border-radius: 9px;
  color: #25351b;
  font: 800 11px/1 ui-monospace, monospace;
}

.topbar-status {
  display: flex;
  align-items: center;
  gap: 8px;
  color: #66746d;
  font: 600 10px/1 ui-monospace, monospace;
  letter-spacing: 0.08em;
}

.status-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: #47a57b;
  box-shadow: 0 0 0 3px #e0f1e7;
}

@media (max-width: 520px) {
  .topbar {
    height: 60px;
  }

  .topbar-status {
    font-size: 9px;
  }
}
</style>
