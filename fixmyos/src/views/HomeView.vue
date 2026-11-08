<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import {
  respondToCommand,
  restoreStartupBackup,
  startRecoveryWorkflow,
  type WorkflowState,
} from '../ws'

const workflow = ref<WorkflowState>({ phase: 'loading', response: 'Starting review...' })
const latestPlan = ref<string[]>([])
const backupBusy = ref(false)
const backupMessage = ref('')
const responseTextElement = ref<HTMLParagraphElement | null>(null)
const responseHasMore = ref(false)
const responseExpanded = ref(false)

function measureResponseOverflow(): void {
  const element = responseTextElement.value
  if (!element || responseExpanded.value) return
  responseHasMore.value = element.scrollHeight > element.clientHeight + 1
}

const responseResizeObserver =
  typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(measureResponseOverflow)

watch(
  responseTextElement,
  async (element, previousElement) => {
    if (previousElement) responseResizeObserver?.unobserve(previousElement)
    if (!element) {
      responseHasMore.value = false
      return
    }
    responseResizeObserver?.observe(element)
    await nextTick()
    measureResponseOverflow()
  },
  { flush: 'post' },
)

watch(
  () => [workflow.value.situation, workflow.value.phase, workflow.value.processNumber],
  async () => {
    responseExpanded.value = false
    await nextTick()
    measureResponseOverflow()
  },
  { flush: 'post' },
)

onMounted(() =>
  startRecoveryWorkflow((state) => {
    workflow.value = state
    if (state.planItems) latestPlan.value = state.planItems
  }),
)
onBeforeUnmount(() => responseResizeObserver?.disconnect())

async function restoreHardBackup(): Promise<void> {
  if (
    !window.confirm(
      'DESTRUCTIVE SYSTEM ROLLBACK\n\nThis restores the origin volume to the daemon-startup snapshot, overwriting changes made since then. The LVM merge consumes the startup snapshot. Continue?',
    )
  ) {
    return
  }

  backupBusy.value = true
  backupMessage.value = 'Restoring the startup snapshot...'
  try {
    const result = await restoreStartupBackup()
    backupMessage.value = result.stdout || 'Startup snapshot restore requested.'
  } catch (error) {
    backupMessage.value = error instanceof Error ? error.message : 'Unable to restore the startup snapshot.'
  } finally {
    backupBusy.value = false
  }
}
</script>

<template>
  <main class="workspace" :class="`workflow--${workflow.phase}`">
    <section class="intro" aria-labelledby="page-title">
      <h1 id="page-title">System recovery <span>Trace the failure. Approve the next step.</span></h1>
      <span class="session-code">LOCAL MACHINE</span>
    </section>

    <div class="console-grid">
      <section class="context-panel" aria-labelledby="analysis-title">
        <div class="panel-heading">
          <div>
            <div class="section-kicker">01 / ANALYSIS</div>
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
            <p
              id="ai-response-description"
              ref="responseTextElement"
              class="situation-copy"
              :class="{ 'situation-copy--expanded': responseExpanded }"
            >
              {{ workflow.situation }}
            </p>
            <button
              v-if="responseHasMore"
              class="response-toggle"
              type="button"
              aria-controls="ai-response-description"
              :aria-expanded="responseExpanded"
              @click="responseExpanded = !responseExpanded"
            >
              {{ responseExpanded ? 'View less' : 'View more' }}
            </button>
          </template>
          <p v-else class="placeholder">No active process description.</p>
        </section>

        <section class="plan-block" aria-labelledby="plan-title">
          <div class="plan-heading">
            <div>
              <div class="section-kicker">02 / CURRENT APPROACH</div>
              <h3 id="plan-title">Latest plan</h3>
            </div>
            <span class="plan-count">{{ String(latestPlan.length).padStart(2, '0') }} STEPS</span>
          </div>
          <ol v-if="latestPlan.length" class="plan-list">
            <li v-for="(item, index) in latestPlan" :key="`${index}-${item}`" class="plan-item">
              <span class="plan-index">{{ String(index + 1).padStart(2, '0') }}</span>
              <span>{{ item }}</span>
            </li>
          </ol>
          <p v-else class="plan-empty">The plan will appear after the process is analyzed.</p>
        </section>
      </section>

      <section class="action-panel" aria-labelledby="action-title">
        <div class="panel-heading">
          <div>
            <div class="section-kicker">03 / ACTION</div>
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
        <div class="hard-restore-control">
          <span class="restore-warning">DESTRUCTIVE ACTION / SYSTEM ROLLBACK</span>
          <button
            class="hard-restore-button"
            type="button"
            :disabled="backupBusy || workflow.phase === 'loading' || workflow.phase === 'running'"
            @click="restoreHardBackup"
          >
            <span class="warning-mark" aria-hidden="true">!</span>
            Restore startup snapshot
          </button>
          <p v-if="backupMessage" class="backup-message" role="status">{{ backupMessage }}</p>
        </div>
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

.intro-actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 20px;
}

.eyebrow,
.section-kicker,
.live-label,
.workspace-footer {
  font: 650 10px/1.2 ui-monospace, monospace;
  letter-spacing: 0;
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

.hard-restore-control {
  display: grid;
  justify-items: end;
  gap: 5px;
}

.restore-warning {
  color: #d78065;
  font: 650 8px/1.2 ui-monospace, 'SFMono-Regular', Consolas, monospace;
}

.hard-restore-button {
  min-height: 30px;
  display: inline-flex;
  align-items: center;
  gap: 7px;
  padding: 0 9px;
  border: 1px solid #764435;
  border-radius: 3px;
  background: #1b1412;
  color: #f08a6b;
  font: 600 10px/1.2 ui-monospace, 'SFMono-Regular', Consolas, monospace;
  cursor: pointer;
  transition: background 160ms ease, border-color 160ms ease, transform 160ms ease;
}

.hard-restore-button:not(:disabled):hover {
  transform: translateY(-1px);
  border-color: #ba6349;
  background: #35201a;
}

.hard-restore-button:focus-visible {
  outline: 2px solid #f08a6b;
  outline-offset: 3px;
}

.hard-restore-button:disabled {
  cursor: not-allowed;
  opacity: 0.5;
}

.warning-mark {
  width: 15px;
  height: 15px;
  display: grid;
  place-items: center;
  border: 1px solid #a85840;
  border-radius: 50%;
  font: 800 9px/1 ui-monospace, monospace;
}

.backup-message {
  max-width: 380px;
  color: #f08a6b;
  font: 10px/1.5 ui-monospace, 'SFMono-Regular', Consolas, monospace;
  text-align: right;
  overflow-wrap: anywhere;
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

  .hard-restore-control {
    justify-items: start;
  }

  .footer-location {
    padding-bottom: 0;
  }

  .backup-message {
    text-align: left;
  }

  .hard-restore-control {
    justify-items: start;
  }

  .backup-message {
    text-align: left;
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
  padding-top: clamp(8px, 1.5vh, 14px);
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
  display: grid;
  grid-template-columns: minmax(0, 1.15fr) minmax(340px, 0.85fr);
  align-items: start;
  gap: 6px;
}

.context-panel,
.action-panel {
  min-width: 0;
  border: 1px solid #303a32;
  border-radius: 4px;
  background: rgba(22, 27, 23, 0.92);
  box-shadow: 0 16px 38px rgba(0, 0, 0, 0.2);
  animation: rise-in 420ms ease-out both;
}

.context-panel {
  padding: 8px 9px 9px;
}

.action-panel {
  position: sticky;
  top: 18px;
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
  padding: 7px 0 8px;
}

.block-label {
  margin: 0 0 6px;
  color: #a0aca2;
  font: 650 9px/1.2 ui-monospace, 'SFMono-Regular', Consolas, monospace;
}

.situation-copy {
  display: -webkit-box;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 4;
  overflow: hidden;
  margin: 0;
  color: #d3ddd4;
  font-size: 13px;
  line-height: 1.75;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.situation-copy--expanded {
  display: block;
  -webkit-line-clamp: unset;
  overflow: visible;
}

.response-content {
  display: contents;
}

.response-toggle {
  margin-top: 12px;
}

.plan-block {
  padding-top: 7px;
  border-top: 1px solid #303a32;
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
  grid-template-columns: 35px minmax(0, 1fr);
  align-items: start;
  gap: 10px;
  padding: 7px 0;
  border-top: 1px solid #272f29;
  color: #d5ded5;
  font-size: 12px;
  line-height: 1.6;
  overflow-wrap: anywhere;
  animation: rise-in 300ms ease-out both;
}

.plan-index {
  color: #dcff68;
  font: 600 10px/1.7 ui-monospace, 'SFMono-Regular', Consolas, monospace;
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
  display: grid;
  gap: 7px;
  min-height: 80px;
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
  margin-top: 8px;
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
  margin-top: 8px;
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
    padding-top: 8px;
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
    grid-template-columns: minmax(0, 1fr);
  }

  .action-panel {
    position: static;
  }
}

@media (max-width: 420px) {
  .workspace {
    width: calc(100% - 10px);
  }

  .context-panel,
  .action-panel {
    padding: 7px;
  }

  .panel-heading h2,
  .plan-heading h3 {
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
