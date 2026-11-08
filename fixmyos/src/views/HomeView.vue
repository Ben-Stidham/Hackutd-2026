<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { getErrors } from '../ws'

const response = ref('')
const requestError = ref('')
const loading = ref(true)
const decision = ref<'yes' | 'no' | null>(null)

onMounted(async () => {
	try {
		response.value = await getErrors()
	} catch (error) {
		requestError.value = error instanceof Error ? error.message : 'Unable to load the analysis.'
	} finally {
		loading.value = false
	}
})
</script>

<template>
  <main class="workspace">
    <section class="intro" aria-labelledby="page-title">
      <div class="eyebrow"><span class="eyebrow-line"></span> SYSTEM RECOVERY</div>
      <h1 id="page-title">A clearer path<br /><span>back to stable.</span></h1>
      <p class="intro-copy">Review the diagnosis, then decide what happens next.</p>
    </section>

    <section class="analysis-section" aria-labelledby="analysis-title">
      <div class="section-heading">
        <div>
          <div class="section-kicker">01 / ANALYSIS</div>
          <h2 id="analysis-title">Nemotron response</h2>
        </div>
        <span class="live-label"><span></span> LIVE OUTPUT</span>
      </div>

      <div class="response-box" :class="{ 'response-box--error': requestError }" aria-live="polite">
        <div class="response-gutter" aria-hidden="true"><span>AI</span></div>
        <div class="response-content">
          <p v-if="loading" class="placeholder"><span class="loading-dot"></span> Reading system report...</p>
          <p v-else-if="requestError" class="error-message">{{ requestError }}</p>
          <p v-else-if="response" class="response-text">{{ response }}</p>
          <p v-else class="placeholder">No response was returned.</p>
        </div>
      </div>
    </section>

    <section class="approval-panel" aria-labelledby="approval-title">
      <div class="approval-icon" aria-hidden="true">?</div>
      <div class="approval-copy">
        <div class="section-kicker">02 / YOUR DECISION</div>
        <h2 id="approval-title">Would you like to run the next command?</h2>
        <p>Review the suggested steps above before making a choice.</p>
        <p v-if="decision" class="decision-status" role="status">
          {{ decision === 'yes' ? 'Yes selected.' : 'No selected.' }}
        </p>
      </div>
      <div class="approval-actions">
        <button class="choice-button choice-button--yes" type="button" :disabled="loading || !!requestError" :aria-pressed="decision === 'yes'" @click="decision = 'yes'">
          Yes <span aria-hidden="true">&#8594;</span>
        </button>
        <button class="choice-button choice-button--no" type="button" :disabled="loading || !!requestError" :aria-pressed="decision === 'no'" @click="decision = 'no'">
          No
        </button>
      </div>
    </section>

    <footer class="workspace-footer">
      <span>FIX MY OS <span class="footer-separator">/</span> REVIEW CONSOLE</span>
      <span>LOCAL MACHINE</span>
    </footer>
  </main>
</template>

<style scoped>
.workspace {
  width: min(100% - 48px, 1000px);
  margin: 0 auto;
  padding: clamp(52px, 9vh, 96px) 0 30px;
  animation: enter 500ms ease-out both;
}

.intro {
  margin-bottom: 54px;
}

.eyebrow,
.section-kicker,
.live-label,
.workspace-footer {
  font: 650 10px/1.2 ui-monospace, monospace;
  letter-spacing: 0.1em;
}

.eyebrow {
  display: flex;
  align-items: center;
  gap: 10px;
  color: #52655a;
}

.eyebrow-line {
  width: 23px;
  height: 2px;
  background: #a4bc4a;
}

h1 {
  margin-top: 21px;
  color: #18251f;
  font: 500 clamp(42px, 6vw, 68px)/0.99 Georgia, 'Times New Roman', serif;
  letter-spacing: 0;
}

h1 span {
  color: #597348;
  font-style: italic;
}

.intro-copy {
  margin-top: 18px;
  color: #65756c;
  font-size: 15px;
}

.analysis-section {
  margin-bottom: 20px;
}

.section-heading {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 16px;
  margin-bottom: 15px;
}

.section-kicker {
  margin-bottom: 8px;
  color: #849188;
}

.section-heading h2,
.approval-copy h2 {
  color: #1d2c24;
  font-size: 19px;
  font-weight: 650;
  letter-spacing: 0;
}

.live-label {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding-bottom: 3px;
  color: #718078;
  white-space: nowrap;
}

.live-label span {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: #5da784;
}

.response-box {
  display: grid;
  grid-template-columns: 58px minmax(0, 1fr);
  min-height: 194px;
  overflow: hidden;
  border: 1px solid #dbe4dd;
  border-radius: 7px;
  background: #fff;
  box-shadow: 0 12px 30px rgba(37, 56, 44, 0.045);
}

.response-box--error {
  border-color: #e5b7a9;
}

.response-gutter {
  display: flex;
  justify-content: center;
  padding-top: 24px;
  border-right: 1px solid #e8ede9;
  background: #f5f8f4;
}

.response-gutter span {
  display: grid;
  width: 26px;
  height: 26px;
  place-items: center;
  border-radius: 50%;
  background: #dceba7;
  color: #40532c;
  font: 750 9px/1 ui-monospace, monospace;
}

.response-content {
  min-width: 0;
  padding: 24px clamp(18px, 4vw, 34px);
}

.response-text {
  color: #334139;
  font: 14px/1.8 ui-monospace, 'SFMono-Regular', Consolas, monospace;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.placeholder {
  display: flex;
  align-items: center;
  gap: 11px;
  color: #849188;
  font-size: 14px;
}

.loading-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #9ab34c;
  animation: pulse 1.2s ease-in-out infinite;
}

.error-message {
  color: #9c4938;
  font-size: 14px;
  line-height: 1.65;
  overflow-wrap: anywhere;
}

.approval-panel {
  display: grid;
  grid-template-columns: 44px minmax(0, 1fr) auto;
  align-items: center;
  gap: 20px;
  padding: 25px 28px;
  border: 1px solid #d6e0d8;
  border-radius: 7px;
  background: #edf3ec;
}

.approval-icon {
  display: grid;
  width: 42px;
  height: 42px;
  place-items: center;
  border: 1px solid #c5d5c7;
  border-radius: 50%;
  color: #47634d;
  font: italic 22px/1 Georgia, serif;
}

.approval-copy h2 {
  margin-bottom: 7px;
  font-size: 17px;
}

.approval-copy > p:not(.decision-status) {
  color: #6a796f;
  font-size: 13px;
}

.decision-status {
  margin-top: 8px;
  color: #4b704e;
  font-size: 12px;
  font-weight: 650;
}

.approval-actions {
  display: flex;
  gap: 9px;
}

.choice-button {
  min-width: 82px;
  height: 42px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 12px;
  padding: 0 16px;
  border: 1px solid transparent;
  border-radius: 5px;
  font-size: 13px;
  font-weight: 650;
  cursor: pointer;
  transition: background 150ms ease, border-color 150ms ease, transform 150ms ease;
}

.choice-button:not(:disabled):active {
  transform: translateY(1px);
}

.choice-button:focus-visible {
  outline: 3px solid #9db756;
  outline-offset: 3px;
}

.choice-button:disabled {
  cursor: not-allowed;
  opacity: 0.48;
}

.choice-button--yes {
  background: #263d2e;
  color: #fff;
}

.choice-button--yes[aria-pressed='true'] {
  background: #587d48;
}

.choice-button--no {
  border-color: #cbd6cc;
  background: #f8faf7;
  color: #415046;
}

.choice-button--no[aria-pressed='true'] {
  border-color: #728675;
  background: #dfe8df;
}

.workspace-footer {
  display: flex;
  justify-content: space-between;
  gap: 16px;
  margin-top: 33px;
  color: #97a39a;
  font-size: 9px;
}

.footer-separator {
  padding: 0 5px;
  color: #bac3bc;
}

@keyframes enter {
  from { opacity: 0; transform: translateY(8px); }
  to { opacity: 1; transform: translateY(0); }
}

@keyframes pulse {
  50% { opacity: 0.35; transform: scale(0.82); }
}

@media (max-width: 700px) {
  .workspace {
    width: min(100% - 36px, 560px);
    padding-top: 50px;
  }

  .intro {
    margin-bottom: 39px;
  }

  .approval-panel {
    grid-template-columns: 38px minmax(0, 1fr);
    gap: 14px;
    padding: 21px 18px;
  }

  .approval-icon {
    width: 36px;
    height: 36px;
  }

  .approval-actions {
    grid-column: 2;
    margin-top: 2px;
  }
}

@media (max-width: 420px) {
  .workspace {
    width: calc(100% - 28px);
  }

  .response-box {
    grid-template-columns: 42px minmax(0, 1fr);
  }

  .response-content {
    padding: 19px 15px;
  }

  .section-heading h2 {
    font-size: 17px;
  }

  .live-label {
    font-size: 8px;
  }
}

@media (prefers-reduced-motion: reduce) {
  *, *::before, *::after {
    animation-duration: 0.01ms !important;
    transition-duration: 0.01ms !important;
  }
}
</style>
