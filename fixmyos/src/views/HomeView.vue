<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import {
  developmentMockMode,
  respondToCommand,
  startMockRecoveryWorkflow,
  startRecoveryWorkflow,
  type WorkflowState,
} from '../ws'

const workflow = ref<WorkflowState>({ phase: 'loading', response: 'Starting review...' })
const latestPlan = ref<string[]>([])
const futureCommands = computed(() => {
  if (!workflow.value.command) return []
  const activeIndex = latestPlan.value.indexOf(workflow.value.command)
  return activeIndex < 0 ? latestPlan.value : latestPlan.value.slice(activeIndex + 1)
})
const pidTotal = ref<number>()
const activePidNumber = ref<number>()
const queueRemaining = computed(() => {
  if (workflow.value.phase === 'complete') return 0
  if (pidTotal.value === undefined || activePidNumber.value === undefined) return undefined
  return Math.max(pidTotal.value - activePidNumber.value + 1, 0)
})
const queueSegmentDash = computed(() => {
  const totalSegments = pidTotal.value ?? 0
  if (totalSegments === 0) return '0 100'

  const segmentUnit = 100 / totalSegments
  const segmentGap = Math.min(1.3, segmentUnit * 0.35)
  return `${segmentUnit - segmentGap} ${segmentGap}`
})
const queueLitSegmentDash = computed(() => {
  const totalSegments = pidTotal.value ?? 0
  if (totalSegments === 0) return '0 100'

  const litSegments = queueRemaining.value ?? 0
  const segmentUnit = 100 / totalSegments
  const segmentGap = Math.min(1.3, segmentUnit * 0.35)
  const segmentLength = segmentUnit - segmentGap
  return Array.from({ length: totalSegments }, (_, index) =>
    index < litSegments ? `${segmentLength} ${segmentGap}` : `0 ${segmentUnit}`,
  ).join(' ')
})
onMounted(() => {
  const onState = (state: WorkflowState) => {
    workflow.value = state
    if (state.planItems) latestPlan.value = state.planItems
    if (state.processCount !== undefined) pidTotal.value = state.processCount
    if (state.processNumber !== undefined) activePidNumber.value = state.processNumber
  }

  if (developmentMockMode) startMockRecoveryWorkflow(onState)
  else startRecoveryWorkflow(onState)
})

</script>

<template>
  <main class="workspace" :class="`workflow--${workflow.phase}`">
    <section class="intro" aria-labelledby="page-title">
      <h1 id="page-title">System recovery <span>Trace the failure. Approve the next step.</span></h1>
      <span class="session-code">{{ developmentMockMode ? 'DEV MOCK / NO COMMANDS EXECUTED' : 'LOCAL MACHINE' }}</span>
    </section>

    <div class="console-grid">
      <section class="context-panel" aria-labelledby="analysis-title">
        <div class="panel-heading">
          <div>
            <div class="section-kicker">01 / CONTEXT</div>
            <h2 id="analysis-title">System context</h2>
          </div>
          <span class="live-label">
            <span></span>
            {{ workflow.processNumber ? `PROCESS ${workflow.processNumber} / ${workflow.processCount}` : workflow.phase.toUpperCase() }}
          </span>
        </div>

        <section class="situation-block" aria-labelledby="situation-title" aria-live="polite">
          <h3 id="situation-title" class="block-label">WHAT'S HAPPENING</h3>
          <p v-if="workflow.phase === 'loading' || workflow.phase === 'running'" class="placeholder">
            <span class="loading-dot"></span>{{ workflow.response }}
          </p>
          <p v-else-if="workflow.phase === 'error'" class="error-message">{{ workflow.error }}</p>
          <template v-else-if="workflow.situation">
            <p class="situation-copy">{{ workflow.situation }}</p>
          </template>
          <p v-else class="placeholder">No active process description.</p>
        </section>
      </section>

      <section class="queue-panel" aria-labelledby="queue-title">
          <div class="queue-heading">
            <div>
              <div class="section-kicker">03 / QUEUE</div>
              <h3 id="queue-title">Processes remaining</h3>
            </div>
            <div class="queue-count" aria-live="polite">
              <strong>{{ queueRemaining ?? '--' }}</strong>
              <span>REMAINING</span>
            </div>
          </div>
          <div class="queue-graphic">
            <svg
              class="queue-ring"
              viewBox="0 0 120 120"
              role="img"
              :aria-label="queueRemaining === undefined ? 'Scanning process queue' : `${queueRemaining} of ${pidTotal ?? 0} process IDs remain, including the active process`"
            >
              <circle
                class="queue-ring-track"
                cx="60"
                cy="60"
                r="47"
                pathLength="100"
                transform="rotate(-90 60 60)"
                :stroke-dasharray="queueSegmentDash"
              />
              <circle
                class="queue-ring-segments"
                cx="60"
                cy="60"
                r="47"
                pathLength="100"
                transform="rotate(-90 60 60)"
                :stroke-dasharray="queueLitSegmentDash"
              />
              <text class="queue-ring-value" x="60" y="58">{{ queueRemaining ?? '--' }}</text>
              <text class="queue-ring-caption" x="60" y="74">LEFT</text>
            </svg>
            <div class="queue-chart-labels">
              <span v-if="workflow.phase === 'complete'">QUEUE CLEAR</span>
              <span v-else-if="workflow.processNumber">ACTIVE {{ workflow.processNumber }} / {{ pidTotal }}</span>
              <span v-else>SCANNING QUEUE</span>
              <span>{{ pidTotal ?? '--' }} TOTAL</span>
            </div>
          </div>
      </section>

      <section class="plan-panel" aria-labelledby="plan-title">
          <div class="plan-heading">
            <div>
              <div class="section-kicker">02 / PLAN</div>
              <h3 id="plan-title">Command timeline</h3>
            </div>
            <span class="plan-count">{{ String((workflow.commandHistory?.length ?? 0) + (workflow.command ? 1 : 0) + futureCommands.length).padStart(2, '0') }} STEPS</span>
          </div>
          <ol v-if="workflow.commandHistory?.length || workflow.command || futureCommands.length" class="plan-list">
            <li v-for="(execution, index) in workflow.commandHistory" :key="`history-${index}-${execution.command}`" class="plan-item plan-item--executed">
              <details class="history-disclosure">
                <summary class="timeline-row">
                  <span class="plan-index">{{ String(index + 1).padStart(2, '0') }}</span>
                  <span class="timeline-state">{{ execution.simulated ? 'SIMULATED' : 'EXECUTED' }}</span>
                  <code>{{ execution.command }}</code>
                  <span class="timeline-arrow" aria-hidden="true"></span>
                </summary>
                <div class="history-output">
                  <div class="output-meta">{{ execution.simulated ? 'MOCK OUTPUT' : `EXIT ${execution.exitCode ?? 'UNKNOWN'}` }}</div>
                  <div v-if="execution.stdout" class="output-stream">
                    <span>STDOUT</span>
                    <pre>{{ execution.stdout }}</pre>
                  </div>
                  <div v-if="execution.stderr" class="output-stream output-stream--error">
                    <span>STDERR</span>
                    <pre>{{ execution.stderr }}</pre>
                  </div>
                  <p v-if="!execution.stdout && !execution.stderr" class="output-empty">No console output.</p>
                </div>
              </details>
            </li>
            <li v-if="workflow.command" class="plan-item plan-item--current">
              <span class="plan-index">{{ String((workflow.commandHistory?.length ?? 0) + 1).padStart(2, '0') }}</span>
              <span class="timeline-state">{{ workflow.phase === 'running' ? 'RUNNING' : 'CURRENT' }}</span>
              <code>{{ workflow.command }}</code>
            </li>
            <li v-for="(item, index) in futureCommands" :key="`future-${index}-${item}`" class="plan-item plan-item--future">
              <span class="plan-index">{{ String((workflow.commandHistory?.length ?? 0) + index + 2).padStart(2, '0') }}</span>
              <span class="timeline-state">UP NEXT</span>
              <code>{{ item }}</code>
            </li>
          </ol>
          <p v-else class="plan-empty">Executed commands and upcoming steps will appear here.</p>
      </section>

      <section class="action-panel" aria-labelledby="action-title">
        <div class="panel-heading">
          <div>
            <div class="section-kicker">04 / ACTION</div>
            <h2 id="action-title">Command review</h2>
          </div>
          <span class="action-state" :class="`action-state--${workflow.phase}`">{{ workflow.phase.toUpperCase() }}</span>
        </div>

        <div class="command-block">
          <div class="block-label">{{ workflow.command ? 'PROPOSED COMMAND' : 'CURRENT STATUS' }}</div>
          <code v-if="workflow.command" class="command-code">{{ workflow.command }}</code>
          <p v-else-if="workflow.phase === 'error'" class="error-message">{{ workflow.error }}</p>
          <p v-else class="command-status">
            <span v-if="workflow.phase === 'loading' || workflow.phase === 'running'" class="loading-dot"></span>
            {{ workflow.response }}
          </p>
        </div>

        <div v-if="workflow.phase === 'approval'" class="approval-controls">
          <p>Run this command?</p>
          <div class="approval-actions">
            <button class="choice-button choice-button--yes" type="button" @click="respondToCommand(true)">
              Approve <span aria-hidden="true">&#8594;</span>
            </button>
            <button class="choice-button choice-button--no" type="button" @click="respondToCommand(false)">
              Decline
            </button>
          </div>
        </div>
        <div v-else class="action-note">
          <span class="action-note-mark" aria-hidden="true">//</span>
          <span>Review the proposed operation before execution.</span>
        </div>
      </section>
    </div>

    <footer class="workspace-footer">
      <span>FIX MY OS <span class="footer-separator">/</span> REVIEW CONSOLE</span>
      <div class="footer-actions">
        <span class="footer-location">LOCAL MACHINE</span>
      </div>
    </footer>
  </main>
</template>

<style scoped>
.workspace {
  width: min(100% - 56px, 1080px);
  margin: 0 auto;
  padding: clamp(42px, 7vh, 76px) 0 34px;
  animation: enter 620ms cubic-bezier(0.2, 0.75, 0.25, 1) both;
}

.intro {
  position: relative;
  margin-bottom: 49px;
  padding-bottom: 30px;
  border-bottom: 1px solid #343c35;
}

.intro-topline {
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.eyebrow {
  display: flex;
  align-items: center;
  gap: 10px;
  color: #b7c1b8;
  font-size: 10px;
}

.eyebrow-line {
  width: 27px;
  height: 3px;
  background: #dcff68;
}

.session-code {
  display: flex;
  align-items: center;
  gap: 9px;
  color: #8d9990;
  font: 600 10px/1 ui-monospace, 'SFMono-Regular', Consolas, monospace;
}

.session-code i {
  width: 1px;
  height: 12px;
  background: #3a433c;
}

h1 {
  margin-top: 23px;
  color: #f0f3ed;
  font: 700 62px/0.94 ui-monospace, 'SFMono-Regular', Consolas, monospace;
  letter-spacing: 0;
  text-transform: uppercase;
}

h1 span {
  color: #dcff68;
}

.intro-copy {
  margin-top: 0;
  color: #a1ada3;
  font-size: 15px;
}

.analysis-section {
  margin-bottom: 18px;
  animation: rise-in 500ms 100ms both;
}

.section-heading {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 16px;
  margin-bottom: 12px;
  padding-bottom: 12px;
  border-bottom: 1px solid #343c35;
}

.section-kicker {
  margin-bottom: 6px;
  color: #869289;
}

.section-heading h2,
.approval-copy h2 {
  color: #e8eee8;
  font-size: 19px;
  font-weight: 650;
  letter-spacing: 0;
}

.live-label {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding-bottom: 3px;
  color: #a4afa6;
  white-space: nowrap;
}

.live-label span {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: #dcff68;
  box-shadow: 0 0 0 3px rgba(220, 255, 104, 0.12);
}

.workflow--loading .live-label span,
.workflow--running .live-label span {
  animation: beacon 1.8s ease-in-out infinite;
}

.workflow--error .live-label span {
  background: #f08a6b;
  box-shadow: 0 0 0 3px rgba(240, 138, 107, 0.12);
}

.response-box {
  position: relative;
  isolation: isolate;
  display: grid;
  grid-template-columns: 54px minmax(0, 1fr);
  min-height: 226px;
  overflow: hidden;
  border: 1px solid #343d36;
  border-radius: 5px;
  background: var(--paper);
  box-shadow: 0 18px 42px rgba(0, 0, 0, 0.24);
  transition: border-color 220ms ease, box-shadow 220ms ease;
}

.response-box::before,
.response-box::after {
  position: absolute;
  z-index: 0;
  inset: 0;
  content: '';
  pointer-events: none;
}

.response-box::before {
  background: repeating-linear-gradient(
    to bottom,
    rgba(220, 255, 104, 0.022) 0,
    rgba(220, 255, 104, 0.022) 1px,
    transparent 1px,
    transparent 4px
  );
}

.response-box::after {
  top: -70px;
  right: 0;
  left: 0;
  bottom: auto;
  height: 70px;
  background: linear-gradient(180deg, transparent, rgba(220, 255, 104, 0.045), transparent);
  animation: scan-pass 8s linear infinite;
}

.response-box--error {
  border-color: #a8543e;
  box-shadow: 0 18px 42px rgba(132, 58, 42, 0.18);
}

.response-gutter {
  position: relative;
  z-index: 1;
  display: flex;
  justify-content: center;
  padding-top: 22px;
  border-right: 1px solid #303a32;
  background: #202720;
}

.response-gutter span {
  display: grid;
  width: 29px;
  height: 29px;
  place-items: center;
  border-radius: 4px;
  background: #dcff68;
  color: #202719;
  font: 800 9px/1 ui-monospace, monospace;
}

.response-content {
  position: relative;
  z-index: 1;
  display: grid;
  align-content: start;
  justify-items: start;
  gap: 12px;
  min-width: 0;
  padding: 25px clamp(18px, 4vw, 36px);
}

.response-text {
  display: -webkit-box;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 4;
  overflow: hidden;
  color: #d9e1d9;
  font: 13px/1.85 ui-monospace, 'SFMono-Regular', Consolas, monospace;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.response-text--expanded {
  display: block;
  -webkit-line-clamp: unset;
  overflow: visible;
}

.response-toggle {
  min-height: 28px;
  padding: 0 9px;
  border: 1px solid #46533c;
  border-radius: 3px;
  background: #171d17;
  color: #dcff68;
  font: 600 10px/1 ui-monospace, 'SFMono-Regular', Consolas, monospace;
  cursor: pointer;
  transition: background 150ms ease, border-color 150ms ease;
}

.response-toggle:hover {
  border-color: #70854b;
  background: #232b20;
}

.response-toggle:focus-visible {
  outline: 2px solid #dcff68;
  outline-offset: 3px;
}

.command-preview {
  display: grid;
  gap: 9px;
  margin-top: 12px;
  padding: 16px 19px;
  border: 1px solid #333d35;
  border-left: 4px solid #dcff68;
  border-radius: 4px;
  background: #0d100e;
  animation: rise-in 360ms ease-out both;
}

.command-label {
  color: #dcff68;
  font: 650 9px/1.2 ui-monospace, monospace;
  letter-spacing: 0;
}

.command-preview code {
  color: #e6ece5;
  font: 13px/1.6 ui-monospace, 'SFMono-Regular', Consolas, monospace;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.placeholder {
  display: flex;
  align-items: center;
  gap: 11px;
  color: #98a49a;
  font-size: 14px;
}

.loading-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #dcff68;
  animation: pulse 1.35s ease-in-out infinite;
}

.workflow--loading .placeholder::after,
.workflow--running .placeholder::after {
  content: '_';
  color: #dcff68;
  animation: caret-blink 900ms steps(2, end) infinite;
}

.error-message {
  color: #f08a6b;
  font-size: 14px;
  line-height: 1.65;
  overflow-wrap: anywhere;
}

.approval-panel {
  display: grid;
  grid-template-columns: 44px minmax(0, 1fr) auto;
  align-items: center;
  gap: 20px;
  padding: 23px 26px;
  border: 1px solid #d1f15f;
  border-radius: 5px;
  background: #dcff68;
  box-shadow: 0 16px 38px rgba(0, 0, 0, 0.24);
  animation: decision-in 420ms cubic-bezier(0.2, 0.75, 0.25, 1) both;
}

.approval-icon {
  display: grid;
  width: 42px;
  height: 42px;
  place-items: center;
  border: 1px solid #657735;
  border-radius: 4px;
  background: #1b2117;
  color: #dcff68;
  font: italic 22px/1 Georgia, serif;
}

.approval-copy h2 {
  margin-bottom: 7px;
  font-size: 17px;
}

.approval-copy > p:not(.decision-status) {
  color: #3e4a3b;
  font-size: 13px;
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
  transition: background 160ms ease, border-color 160ms ease, color 160ms ease, transform 160ms ease;
}

.choice-button:not(:disabled):active {
  transform: translateY(1px);
}

.choice-button:focus-visible {
  outline: 3px solid #dcff68;
  outline-offset: 3px;
}

.choice-button:disabled {
  cursor: not-allowed;
  opacity: 0.48;
}

.choice-button--yes {
  background: #171c15;
  color: #eff5e8;
}

.choice-button--yes[aria-pressed='true'] {
  background: #303a27;
}

.choice-button--no {
  border-color: #65704f;
  background: transparent;
  color: #263020;
}

.choice-button--no[aria-pressed='true'] {
  border-color: #53613d;
  background: rgba(31, 40, 25, 0.14);
}

.workspace-footer {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 16px;
  margin-top: 27px;
  padding-top: 14px;
  border-top: 1px solid #343c35;
  color: #7f8b82;
  font-size: 9px;
}

.footer-actions {
  display: flex;
  align-items: flex-end;
  gap: 22px;
}

.footer-location {
  padding-bottom: 8px;
}

.footer-separator {
  padding: 0 5px;
  color: #59645b;
}

@keyframes enter {
  from { opacity: 0; transform: translateY(8px); }
  to { opacity: 1; transform: translateY(0); }
}

@keyframes pulse {
  50% { opacity: 0.35; transform: scale(0.82); }
}

@keyframes beacon {
  50% { opacity: 0.45; box-shadow: 0 0 0 6px rgba(220, 255, 104, 0.035); }
}

@keyframes caret-blink {
  50% { opacity: 0; }
}

@keyframes scan-pass {
  from { top: -70px; }
  to { top: 100%; }
}

@keyframes rise-in {
  from { opacity: 0; transform: translateY(9px); }
  to { opacity: 1; transform: translateY(0); }
}

@keyframes decision-in {
  from { opacity: 0; transform: translateY(10px) scale(0.99); }
  to { opacity: 1; transform: translateY(0) scale(1); }
}

@media (max-width: 700px) {
  .workspace {
    width: min(100% - 36px, 620px);
    padding-top: 38px;
  }

  .intro {
    margin-bottom: 32px;
    padding-bottom: 24px;
  }

  h1 {
    font-size: 46px;
  }

  .intro-actions {
    align-items: flex-start;
    flex-direction: column;
    gap: 14px;
  }

  .workspace-footer {
    align-items: flex-start;
    flex-direction: column;
    gap: 18px;
  }

  .footer-actions {
    width: 100%;
    align-items: flex-start;
    flex-direction: column;
    gap: 12px;
  }

  .footer-location {
    padding-bottom: 0;
  }

  .approval-panel {
    grid-template-columns: 38px minmax(0, 1fr);
    gap: 14px;
    padding: 19px 16px;
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

  .session-code {
    gap: 6px;
    font-size: 8px;
  }

  h1 {
    font-size: 39px;
  }

  .section-heading {
    align-items: flex-start;
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

  .approval-actions {
    width: 100%;
  }

  .choice-button {
    flex: 1;
  }
}

.workspace {
  width: min(100% - 16px, 1360px);
  min-height: 100svh;
  display: flex;
  flex-direction: column;
  padding-top: clamp(8px, 1.5vh, 14px);
  padding-bottom: 8px;
}

.intro {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 10px;
  margin-bottom: 8px;
  padding-bottom: 6px;
  border-bottom-color: #303831;
}

h1 {
  display: flex;
  align-items: baseline;
  flex-wrap: wrap;
  gap: 12px 20px;
  margin: 0;
  font: 700 22px/1.2 ui-monospace, 'SFMono-Regular', Consolas, monospace;
}

h1 span {
  color: #8f9b91;
  font: 400 12px/1.5 'Avenir Next', 'Segoe UI', sans-serif;
  text-transform: none;
}

.session-code {
  flex: 0 0 auto;
  font-size: 9px;
}

.console-grid {
  flex: 1;
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
  grid-template-rows: minmax(0, 2fr) minmax(210px, 1fr);
  grid-template-areas:
    'context plan'
    'queue action';
  align-items: stretch;
  min-height: 0;
  gap: 6px;
}

.context-panel,
.queue-panel,
.plan-panel,
.action-panel {
  min-width: 0;
  min-height: 0;
  overflow: auto;
  border: 1px solid #303a32;
  border-radius: 4px;
  background: rgba(22, 27, 23, 0.92);
  box-shadow: 0 16px 38px rgba(0, 0, 0, 0.2);
  animation: rise-in 420ms ease-out both;
}

.context-panel {
  grid-area: context;
  display: flex;
  flex-direction: column;
  padding: 8px 9px 9px;
}

.queue-panel {
  grid-area: queue;
  display: flex;
  flex-direction: column;
  justify-content: center;
  padding: 14px;
}

.plan-panel {
  grid-area: plan;
  padding: 12px;
}

.action-panel {
  grid-area: action;
  align-self: stretch;
  min-height: 0;
  display: flex;
  flex-direction: column;
  padding: 8px;
  animation-delay: 80ms;
}

.panel-heading,
.plan-heading {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 14px;
}

.panel-heading {
  padding-bottom: 7px;
  border-bottom: 1px solid #303a32;
}

.panel-heading h2,
.plan-heading h3 {
  margin: 0;
  color: #e8eee8;
  font-size: 16px;
  font-weight: 650;
}

.section-kicker {
  margin: 0 0 6px;
  color: #8c988e;
  font-size: 9px;
}

.live-label {
  padding-top: 5px;
  font-size: 9px;
}

.situation-block {
  min-height: clamp(190px, 27svh, 290px);
  padding: 14px 0 18px;
}

.block-label {
  margin: 0 0 6px;
  color: #a0aca2;
  font: 650 9px/1.2 ui-monospace, 'SFMono-Regular', Consolas, monospace;
}

.situation-copy {
  margin: 0;
  color: #d3ddd4;
  font-size: 14px;
  line-height: 1.7;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.queue-heading {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.queue-heading h3 {
  margin: 0;
  color: #e8eee8;
  font-size: 15px;
  font-weight: 650;
}

.queue-count {
  display: flex;
  align-items: baseline;
  gap: 8px;
}

.queue-count strong {
  color: #dcff68;
  font: 650 25px/1 ui-monospace, 'SFMono-Regular', Consolas, monospace;
}

.queue-count span,
.queue-chart-labels {
  color: #849087;
  font: 600 8px/1.3 ui-monospace, 'SFMono-Regular', Consolas, monospace;
}

.queue-graphic {
  flex: 1;
  display: grid;
  align-content: center;
  justify-items: center;
  gap: 12px;
  padding-top: 12px;
}

.queue-ring {
  display: block;
  width: clamp(88px, 12svh, 124px);
  aspect-ratio: 1;
  overflow: visible;
}

.queue-ring-track,
.queue-ring-segments {
  fill: none;
  stroke-width: 5;
}

.queue-ring-track {
  stroke: #303a32;
}

.queue-ring-segments {
  stroke: #dcff68;
  stroke-linecap: butt;
  filter: drop-shadow(0 0 3px rgba(220, 255, 104, 0.2));
  transition: stroke-dasharray 350ms ease;
}

.queue-ring-value,
.queue-ring-caption {
  text-anchor: middle;
  font-family: ui-monospace, 'SFMono-Regular', Consolas, monospace;
}

.queue-ring-value {
  fill: #dcff68;
  font-size: 25px;
  font-weight: 700;
}

.queue-ring-caption {
  fill: #849087;
  font-size: 7px;
  font-weight: 600;
}

.queue-chart-labels {
  display: flex;
  justify-content: space-between;
  gap: 12px;
}

.response-content {
  display: contents;
}

.response-toggle {
  margin-top: 12px;
}

.plan-block {
  min-height: 100%;
  padding-top: 7px;
}

.plan-heading {
  align-items: center;
  margin-bottom: 7px;
}

.plan-count {
  color: #849087;
  font: 600 9px/1.2 ui-monospace, 'SFMono-Regular', Consolas, monospace;
}

.plan-list {
  display: grid;
  gap: 0;
  margin: 0;
  padding: 0;
  list-style: none;
  counter-reset: plan-step;
}

.plan-item {
  display: grid;
  grid-template-columns: 30px 72px minmax(0, 1fr) 14px;
  align-items: start;
  gap: 8px;
  padding: 8px 6px;
  border-top: 1px solid #272f29;
  color: #d5ded5;
  font-size: 12px;
  line-height: 1.55;
  overflow-wrap: anywhere;
  animation: rise-in 300ms ease-out both;
}

.plan-item--current {
  border: 1px solid #71844a;
  border-left: 3px solid #dcff68;
  border-radius: 3px;
  background: rgba(220, 255, 104, 0.07);
  color: #eff5e8;
}

.plan-item--future {
  margin-left: 22px;
  opacity: 0.72;
}

.history-disclosure {
  grid-column: 1 / -1;
  min-width: 0;
}

.timeline-row {
  display: grid;
  grid-template-columns: 30px 72px minmax(0, 1fr) 14px;
  align-items: start;
  gap: 8px;
  cursor: pointer;
  list-style: none;
}

.timeline-row::-webkit-details-marker {
  display: none;
}

.timeline-row code,
.plan-item > code {
  min-width: 0;
  color: inherit;
  font: 11px/1.6 ui-monospace, 'SFMono-Regular', Consolas, monospace;
  overflow-wrap: anywhere;
}

.timeline-state {
  padding-top: 2px;
  color: #8d9990;
  font: 600 8px/1.4 ui-monospace, 'SFMono-Regular', Consolas, monospace;
}

.plan-item--current .timeline-state {
  color: #dcff68;
}

.timeline-arrow {
  width: 7px;
  height: 7px;
  margin: 4px 3px 0 0;
  border-right: 1px solid #aeb8af;
  border-bottom: 1px solid #aeb8af;
  transform: rotate(45deg);
  transition: transform 150ms ease;
}

.history-disclosure[open] .timeline-arrow {
  transform: rotate(225deg);
}

.history-output {
  display: grid;
  gap: 8px;
  margin: 10px 0 2px 38px;
  padding: 10px;
  border-left: 1px solid #46533c;
  background: #0c100d;
}

.output-meta,
.output-stream > span {
  color: #849087;
  font: 600 8px/1.3 ui-monospace, 'SFMono-Regular', Consolas, monospace;
}

.output-stream pre {
  max-height: 200px;
  overflow: auto;
  margin: 4px 0 0;
  color: #cbd6cc;
  font: 10px/1.55 ui-monospace, 'SFMono-Regular', Consolas, monospace;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.output-stream--error pre {
  color: #f08a6b;
}

.output-empty {
  margin: 0;
  color: #78847b;
  font-size: 10px;
}

.plan-index {
  color: #dcff68;
  font: 600 11px/1.7 ui-monospace, 'SFMono-Regular', Consolas, monospace;
}

.plan-empty {
  margin: 0;
  color: #7f8b82;
  font-size: 12px;
}

.action-state {
  padding: 5px 7px;
  border: 1px solid #3c473e;
  border-radius: 2px;
  color: #a8b4aa;
  font: 600 8px/1 ui-monospace, 'SFMono-Regular', Consolas, monospace;
}

.action-state--approval {
  border-color: #77894c;
  color: #dcff68;
}

.action-state--error {
  border-color: #8c4c3a;
  color: #f08a6b;
}

.command-block {
  flex: 1 1 auto;
  display: grid;
  gap: 7px;
  min-height: 84px;
  align-content: start;
  margin-top: 8px;
  padding: 9px;
  border: 1px solid #303a32;
  border-left: 3px solid #dcff68;
  border-radius: 3px;
  background: #0c100d;
}

.command-code {
  color: #e4ece4;
  font: 12px/1.8 ui-monospace, 'SFMono-Regular', Consolas, monospace;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.command-status {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  margin: 0;
  color: #a7b2a8;
  font: 12px/1.7 ui-monospace, 'SFMono-Regular', Consolas, monospace;
  overflow-wrap: anywhere;
}

.command-status .loading-dot {
  flex: 0 0 auto;
  margin-top: 5px;
}

.approval-controls {
  display: grid;
  gap: 7px;
  margin-top: auto;
  padding-top: 8px;
  border-top: 1px solid #303a32;
}

.approval-controls > p {
  margin: 0;
  color: #c1ccc2;
  font-size: 12px;
}

.approval-actions {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 8px;
}

.choice-button {
  min-width: 0;
  height: 40px;
  justify-content: space-between;
  border-radius: 3px;
  font: 650 11px/1 ui-monospace, 'SFMono-Regular', Consolas, monospace;
}

.choice-button--yes {
  background: #dcff68;
  color: #1d2518;
}

.choice-button--yes:hover {
  background: #e7ff9a;
}

.choice-button--no {
  border-color: #48534a;
  background: #1b211c;
  color: #d0dad1;
}

.choice-button--no:hover {
  border-color: #7a897d;
  background: #252d26;
}

.action-note {
  display: flex;
  gap: 5px;
  margin-top: 16px;
  padding-top: 8px;
  color: #7f8b82;
  font-size: 11px;
  line-height: 1.5;
}

.action-note-mark {
  color: #dcff68;
  font-family: ui-monospace, 'SFMono-Regular', Consolas, monospace;
}

.workspace-footer {
  margin-top: 7px;
}

@media (max-width: 760px) {
  .workspace {
    width: min(100% - 12px, 720px);
    min-height: 100svh;
    padding-top: 8px;
    padding-bottom: 8px;
  }

  .intro {
    align-items: flex-start;
    flex-direction: column;
    gap: 4px;
    margin-bottom: 7px;
    padding-bottom: 5px;
  }

  h1 {
    gap: 4px 12px;
    font-size: 19px;
  }

  .session-code {
    font-size: 8px;
  }

  .console-grid {
    flex: initial;
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: auto;
    grid-template-areas:
      'context'
      'queue'
      'plan'
      'action';
    min-height: 0;
    gap: 6px;
  }

  .context-panel,
  .queue-panel,
  .plan-panel,
  .action-panel {
    min-height: 0;
    overflow: visible;
  }

  .situation-block {
    min-height: 0;
    padding: 12px 0 14px;
  }

  .action-panel {
    position: static;
    align-self: stretch;
    min-height: 0;
  }

  .queue-panel {
    min-height: 120px;
  }

  .command-block {
    flex: initial;
    min-height: 110px;
  }

  .approval-controls,
  .action-note {
    margin-top: 8px;
  }
}

@media (max-width: 420px) {
  .workspace {
    width: calc(100% - 10px);
  }

  .context-panel,
  .queue-panel,
  .plan-panel,
  .action-panel {
    padding: 7px;
  }

  .panel-heading h2,
  .plan-heading h3,
  .queue-heading h3 {
    font-size: 15px;
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
