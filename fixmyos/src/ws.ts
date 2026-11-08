import agentInstructions from '../../agent_instructions.md?raw'
import { io } from 'socket.io-client'
import { Nemotron } from './nemotron'

export type WorkflowPhase = 'loading' | 'approval' | 'running' | 'complete' | 'error'

export type ExecutedCommand = {
	command: string
	exitCode?: number
	stdout: string
	stderr: string
	simulated?: boolean
}

export type WorkflowState = {
	phase: WorkflowPhase
	response: string
	processName?: string
	commandHistory?: ExecutedCommand[]
	situation?: string
	planItems?: string[]
	processNumber?: number
	processCount?: number
	command?: string
	error?: string
}

type StateListener = (state: WorkflowState) => void

type MockProcess = {
	processName: string
	situation: string
	commands: string[]
}

type RestoreResponse = {
	success?: boolean
	stdout?: string
	stderr?: string
	error?: string
	message?: string
}

const nemotron = new Nemotron()
export const developmentMockMode =
	import.meta.env.DEV && new URLSearchParams(window.location.search).get('mock-ai') === '1'

const socket = io(import.meta.env.VITE_SOCKET_URL || window.location.origin, {
	path: '/socket.io',
	autoConnect: !developmentMockMode,
})

const MAX_FORMAT_REPHRASE_ATTEMPTS = 2

let stateListener: StateListener | undefined
let processIds: string[] = []
let currentProcessIndex = 0
let currentProcessName: string | undefined
let currentCommand: string | undefined
let commandHistory: ExecutedCommand[] = []
let mockMode = false
let mockProcessIndex = 0
let mockCommandIndex = 0

const mockProcesses: MockProcess[] = [
	{
		processName: 'sync-agent',
		situation:
			'Lorem ipsum dolor sit amet, consectetur adipiscing elit. A background sync worker exited unexpectedly after a retry cycle. Its captured output suggests a missing executable or an incorrect service environment. I am reviewing the available process context and will propose inspection steps before any repair action.',
		commands: [
			'journalctl -u sync-agent.service --since "30 minutes ago" --no-pager',
			'systemctl status sync-agent.service --no-pager',
			'systemctl cat sync-agent.service',
		],
	},
	{
		processName: 'storage-worker',
		situation:
			'Curabitur sodales ligula in libero. A second sample process reported a storage-related failure. The first inspection narrowed the issue to the mount configuration; I am checking the affected path and its current mount source before suggesting a change.',
		commands: ['findmnt -T /var/lib/example', 'df -h /var/lib/example'],
	},
]

function promptJson(value: unknown, maxChars = 14_000): string {
	const json = JSON.stringify(value, null, 2)
	return json.length > maxChars
		? `${json.slice(0, maxChars)}\n[Output truncated to fit Nemotron's context window.]`
		: json
}

socket.on('connect_error', (error) => {
	console.error('Socket.IO connection failed:', error)
})

export function startRecoveryWorkflow(onState: StateListener): void {
	mockMode = false
	stateListener = onState
	processIds = []
	currentProcessIndex = 0
	currentProcessName = undefined
	currentCommand = undefined
	commandHistory = []
	nemotron.reset()
	void findProcesses()
}

export function startMockRecoveryWorkflow(onState: StateListener): void {
	mockMode = true
	stateListener = onState
	mockProcessIndex = 0
	mockCommandIndex = 0
	commandHistory = []
	showMockStep()
}

export async function restoreStartupBackup(): Promise<RestoreResponse> {
	if (mockMode) {
		return {
			success: true,
			stdout: 'Development preview only. No system files were restored.',
			stderr: '',
		}
	}

	return new Promise((resolve, reject) => {
		const timeout = window.setTimeout(() => {
			reject(new Error('Timed out waiting for the startup restore acknowledgement.'))
		}, 120_000)

		socket.emit('restore_backup', { target: 'startup' }, (response: RestoreResponse) => {
			window.clearTimeout(timeout)
			if (response?.success) {
				resolve(response)
				return
			}
			reject(new Error(response?.stderr || response?.error || response?.message || 'Startup restore failed.'))
		})
	})
}

export async function respondToCommand(approved: boolean): Promise<void> {
	if (mockMode) {
		await respondToMockCommand(approved)
		return
	}

	if (!approved) {
		updateState({ phase: 'running', response: 'Skipping this process...' })
		currentProcessIndex++
		commandHistory = []
		await loadCurrentProcess()
		return
	}
	if (!currentCommand) return

	const command = currentCommand
	currentCommand = undefined
	updateState({ phase: 'running', response: 'Running the approved command...', command })

	try {
		const result = await requestSocket<unknown>('run_command', 'command_result', { command })
		commandHistory.push(toExecutedCommand(command, result))
		updateState({ phase: 'running', response: 'Reviewing command output...' })
		const answer = await nemotron.ask(
			`${agentInstructions}\n\nCommand result:\n${promptJson(result, 10_000)}\n\n` +
				'Continue analyzing this process. Start with a short `### Situation` section explaining what is happening and what you are doing. ' +
				'For the next step, put exactly one shell command on a line beginning with `## `; do not include any later commands. ' +
				'When no more action is required, end with the exact text `FIX COMPLETE` on its own line. ' +
				'Do not put the command heading in a code fence.',
		)
		await showModelStep(answer)
	} catch (error) {
		updateState({
			phase: 'error',
			response: '',
			command,
			error: error instanceof Error ? error.message : 'The approved command failed.',
		})
	}
}

async function respondToMockCommand(approved: boolean): Promise<void> {
	const process = mockProcesses[mockProcessIndex]
	if (!process) return
	const command = process.commands[mockCommandIndex]

	updateState({
		phase: 'running',
		response: approved
			? `Simulating command ${mockCommandIndex + 1} of ${process.commands.length}. No command was executed.`
			: 'Skipping this process. No command was executed.',
		processNumber: mockProcessIndex + 1,
		processCount: mockProcesses.length,
		command: approved ? command : undefined,
	})
	await new Promise((resolve) => window.setTimeout(resolve, 650))

	if (approved && command) {
		commandHistory.push({
			command,
			exitCode: 0,
			stdout: `Mock output for: ${command}\nLorem ipsum dolor sit amet, consectetur adipiscing elit.`,
			stderr: '',
			simulated: true,
		})
		mockCommandIndex++
	}
	else mockCommandIndex = process.commands.length

	if (mockCommandIndex >= process.commands.length) {
		mockProcessIndex++
		mockCommandIndex = 0
		commandHistory = []
	}
	showMockStep()
}

function showMockStep(): void {
	const process = mockProcesses[mockProcessIndex]
	if (!process) {
		currentProcessName = undefined
		updateState({
			phase: 'complete',
			response: 'All development scenarios have been reviewed.',
			situation: 'Mock workflow complete. No system commands or backups were run.',
			planItems: [],
			processNumber: mockProcesses.length,
			processCount: mockProcesses.length,
		})
		return
	}

	currentProcessName = process.processName
	const approvedCommands = process.commands.slice(0, mockCommandIndex)
	const activeCommand = process.commands[mockCommandIndex]
	const remainingCommands = process.commands.slice(mockCommandIndex)
	updateState({
		phase: 'approval',
		response: process.situation,
		situation:
			mockCommandIndex > 0
				? `${process.situation} Mock step ${mockCommandIndex + 1} is ready; ${approvedCommands.length} prior command(s) were simulated.`
				: process.situation,
		planItems: remainingCommands,
		processNumber: mockProcessIndex + 1,
		processCount: mockProcesses.length,
		command: activeCommand,
	})
}

async function findProcesses(): Promise<void> {
	updateState({ phase: 'loading', response: 'Checking reported process errors...' })
	console.log("Starting process discovery...")
	try {
		const summary = await requestSocket<unknown>('get_errors', 'errors_list')
		console.log("Process discovery complete. Summary:", summary)
		const answer = await nemotron.ask(
			`${agentInstructions}\n\nInitial process list:\n${promptJson(summary)}\n\n` +
				'Return the requested process IDs exactly as process_list: ["id", ...]. Do not display or explain the IDs.',
		)
		console.log("Nemotron response for process IDs:", answer)
		processIds = parseProcessIds(answer)
		console.log("Parsed process IDs:", processIds)
		if (processIds.length === 0) {
			updateState({ phase: 'complete', response: 'No processes need investigation.' })
			return
		}
		await loadCurrentProcess()
	} catch (error) {
		updateState({
			phase: 'error',
			response: '',
			error: error instanceof Error ? error.message : 'Unable to analyze the process list.',
		})
	}
}

async function loadCurrentProcess(): Promise<void> {
	console.log("Loading process index:", currentProcessIndex, "of", processIds.length)
	currentCommand = undefined
	currentProcessName = undefined
	if (currentProcessIndex >= processIds.length) {
		updateState({ phase: 'complete', response: 'All selected processes have been reviewed.' })
		return
	}

	updateState({
		phase: 'loading',
		response: 'Loading process details...',
		processNumber: currentProcessIndex + 1,
		processCount: processIds.length,
	})

	try {
		nemotron.reset()
		const processId = processIds[currentProcessIndex]
		updateState({
			phase: 'loading',
			response: 'Loading process details...',
			processNumber: currentProcessIndex + 1,
			processCount: processIds.length,
		})
		const details = await requestSocket<unknown>('get_error', 'error_details', { id: processId })
		currentProcessName = extractProcessName(details)
		updateState({
			phase: 'loading',
			response: 'Analyzing process details...',
			processNumber: currentProcessIndex + 1,
			processCount: processIds.length,
		})
		const answer = await nemotron.ask(
			`${agentInstructions}\n\nAdditional information for process ID ${processId}:\n` +
				`${promptJson(details)}\n\n` +
				'Analyze this process and follow the required output format. Start with a short `### Situation` section explaining what is happening and what you are doing. ' +
				'For the next step, put exactly one shell command on a line beginning with `## `; do not include any later commands. ' +
				'When no more action is required, end with the exact text `FIX COMPLETE` on its own line. ' +
				'Do not put the command heading in a code fence.',
		)
		await showModelStep(answer)
	} catch (error) {
		updateState({
			phase: 'error',
			response: '',
			processNumber: currentProcessIndex + 1,
			processCount: processIds.length,
			error: error instanceof Error ? error.message : 'Unable to load process details.',
		})
	}
}

async function showModelStep(answer: string): Promise<void> {
	let next = getNextCommand(answer)
	for (let attempt = 0; next.kind === 'missing' && attempt < MAX_FORMAT_REPHRASE_ATTEMPTS; attempt++) {
		updateState({
			phase: 'running',
			response: 'Asking Nemotron to rephrase its response...',
			processNumber: currentProcessIndex + 1,
			processCount: processIds.length,
		})
		answer = await nemotron.ask(
			'Your previous response did not follow the required output format. Rephrase it without changing its analysis or recommendation, and do not invent a new command. Return only one of these formats:\n' +
				'For another action: a short `### Situation` section, followed by exactly one shell command on a line beginning with `## `. Do not include any later commands.\n' +
				'When no more action is required: end with the exact text `FIX COMPLETE` on its own line.\n' +
				'Do not put the command heading in a code fence.\n\nPrevious response:\n' +
				answer,
		)
		next = getNextCommand(answer)
	}

	if (next.kind === 'done') {
		currentProcessIndex++
		commandHistory = []
		await loadCurrentProcess()
		return
	}
	if (next.kind === 'missing') {
		commandHistory = []
		updateState({
			phase: 'error',
			response: answer,
			error: 'Nemotron did not include a command heading or the FIX COMPLETE marker.',
			processNumber: currentProcessIndex + 1,
			processCount: processIds.length,
		})
		return
	}

	currentCommand = next.command
	const situation = extractSection(answer, 'Situation')
	const planItems = extractPlanItems(answer)
	updateState({
		phase: 'approval',
		response: answer,
		situation: situation || answerBeforeCommand(answer),
		planItems,
		processNumber: currentProcessIndex + 1,
		processCount: processIds.length,
		command: next.command,
	})
}

function extractSection(answer: string, sectionName: string): string {
	const escapedName = sectionName.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
	const heading = new RegExp(`^#{2,3}\\s+${escapedName}\\s*$`, 'im').exec(answer)
	if (!heading) return ''

	const contentStart = heading.index + heading[0].length
	const content = answer.slice(contentStart)
	const nextHeading = content.search(/^#{1,3}\s+/m)
	return content.slice(0, nextHeading < 0 ? undefined : nextHeading).trim()
}

function extractPlanItems(answer: string): string[] {
	return [...answer.matchAll(/^##\s+(.+?)\s*$/gim)]
		.map((match) => match[1]?.trim().replace(/^`|`$/g, '') ?? '')
		.filter((command) => command.length > 0 && !/^(?:done|no more commands?)\b/i.test(command))
}

function answerBeforeCommand(answer: string): string {
	return (answer.split(/^##\s+/m, 1)[0] ?? answer)
		.replace(/^#.+$/gm, '')
		.replace(/^###\s+(?:Situation|Plan)\s*$/gim, '')
		.trim()
}

function parseProcessIds(answer: string): string[] {
	const match = answer.match(/process_list\s*:\s*(\[[\s\S]*?\])/i)
	if (!match) {
		throw new Error('Nemotron did not return a process_list array.')
	}

	const processListJson = match[1]
	if (!processListJson) {
		throw new Error('Nemotron returned an empty process_list.')
	}
	const parsed: unknown = JSON.parse(processListJson)
	if (!Array.isArray(parsed)) {
		throw new Error('Nemotron returned an invalid process_list.')
	}
	return parsed.map((id) => String(id)).filter((id) => id.length > 0)
}

function getNextCommand(
	answer: string,
): { kind: 'command'; command: string } | { kind: 'done' } | { kind: 'missing' } {
	if (/^(?:##\s*)?FIX COMPLETE\s*$/im.test(answer)) return { kind: 'done' }

	const heading = answer.match(/^##\s+(.+?)\s*$/im)?.[1]?.trim()
	if (!heading) return { kind: 'missing' }
	if (/^(?:done|no more commands?)\b/i.test(heading)) return { kind: 'missing' }

	const command = heading.replace(/^`|`$/g, '').trim()
	if (!command || /^<.*>$/.test(command)) return { kind: 'missing' }
	return { kind: 'command', command }
}

function requestSocket<T>(event: string, responseEvent: string, payload?: unknown): Promise<T> {
	return new Promise((resolve, reject) => {
		const timeout = window.setTimeout(() => {
			socket.off(responseEvent, onResponse)
			reject(new Error(`Timed out waiting for the ${responseEvent} Socket.IO message.`))
		}, 120_000)

		const onResponse = (response: T) => {
			window.clearTimeout(timeout)
			resolve(response)
		}

		socket.once(responseEvent, onResponse)
		if (payload === undefined) socket.emit(event)
		else socket.emit(event, payload)
	})
}

function updateState(state: WorkflowState): void {
	console.log(state.phase.toUpperCase(), state.response, state.processNumber ? `(${state.processNumber}/${state.processCount})` : '')
	stateListener?.({ ...state, processName: currentProcessName, commandHistory: [...commandHistory] })
}

function extractProcessName(details: unknown): string | undefined {
	if (typeof details !== 'object' || details === null || Array.isArray(details)) return undefined

	const data = details as Record<string, unknown>
	const name = data.process_name ?? data.comm
	return typeof name === 'string' && name.trim() ? name.trim() : undefined
}

function toExecutedCommand(command: string, result: unknown): ExecutedCommand {
	const data = typeof result === 'object' && result !== null ? (result as Record<string, unknown>) : {}
	const rawExitCode = data.exit_code ?? data.exitCode
	const parsedExitCode = typeof rawExitCode === 'number' ? rawExitCode : Number(rawExitCode)

	return {
		command,
		exitCode: Number.isFinite(parsedExitCode) ? parsedExitCode : undefined,
		stdout: typeof data.stdout === 'string' ? data.stdout : '',
		stderr: typeof data.stderr === 'string' ? data.stderr : '',
	}
}
