import agentInstructions from '../../agent_instructions.md?raw'
import { io } from 'socket.io-client'
import { Nemotron } from './nemotron'

export type WorkflowPhase = 'loading' | 'approval' | 'running' | 'complete' | 'error'

export type WorkflowState = {
	phase: WorkflowPhase
	response: string
	processNumber?: number
	processCount?: number
	command?: string
	error?: string
}

type StateListener = (state: WorkflowState) => void

type BackupResponse = {
	success?: boolean
	snapshot_name?: string
	stdout?: string
	stderr?: string
	error?: string
	message?: string
}

const nemotron = new Nemotron()
const socket = io(import.meta.env.VITE_SOCKET_URL || window.location.origin, {
	path: '/socket.io',
})

let stateListener: StateListener | undefined
let processIds: string[] = []
let currentProcessIndex = 0
let currentCommand: string | undefined

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
	stateListener = onState
	processIds = []
	currentProcessIndex = 0
	currentCommand = undefined
	nemotron.reset()
	void findProcesses()
}

export async function restoreStartupBackup(): Promise<BackupResponse> {
	return requestBackup('restore_backup', { target: 'startup' })
}

export async function respondToCommand(approved: boolean): Promise<void> {
	if (!approved) {
		updateState({ phase: 'running', response: 'Restoring the soft backup...' })
		try {
			await requestBackup('restore_backup')
		} catch (error) {
			updateState({
				phase: 'error',
				response: '',
				error: error instanceof Error ? error.message : 'Unable to restore the soft backup.',
				processNumber: currentProcessIndex + 1,
				processCount: processIds.length,
			})
			return
		}
		currentProcessIndex++
		await loadCurrentProcess()
		return
	}
	if (!currentCommand) return

	const command = currentCommand
	currentCommand = undefined
	updateState({ phase: 'running', response: 'Running the approved command...' })

	try {
		const result = await requestSocket<unknown>('run_command', 'command_result', { command })
		const answer = await nemotron.ask(
			`${agentInstructions}\n\nCommand result:\n${promptJson(result, 10_000)}\n\n` +
				'Continue analyzing this process. If another command is needed, put only the next command in a level-two heading (## command). ' +
				'If no more commands are needed for this process, include the exact heading ## DONE.',
		)
		await showModelStep(answer)
	} catch (error) {
		updateState({
			phase: 'error',
			response: '',
			error: error instanceof Error ? error.message : 'The approved command failed.',
		})
	}
}

async function findProcesses(): Promise<void> {
	updateState({ phase: 'loading', response: 'Checking reported process errors...' })

	try {
		const summary = await requestSocket<unknown>('get_errors', 'errors_list')
		const answer = await nemotron.ask(
			`${agentInstructions}\n\nInitial process list:\n${promptJson(summary)}\n\n` +
				'Return the requested process IDs exactly as process_list: ["id", ...]. Do not display or explain the IDs.',
		)
		processIds = parseProcessIds(answer)
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
	currentCommand = undefined
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
		await requestBackup('create_backup')
		const details = await requestSocket<unknown>('get_error', 'error_details', { id: processId })
		const answer = await nemotron.ask(
			`${agentInstructions}\n\nAdditional information for process ID ${processId}:\n` +
				`${promptJson(details)}\n\n` +
				'Analyze this process and follow the required output format. If no command is needed, include the exact heading ## DONE.',
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
	const next = getNextCommand(answer)
	if (next.kind === 'done') {
		currentProcessIndex++
		await loadCurrentProcess()
		return
	}
	if (next.kind === 'missing') {
		updateState({
			phase: 'error',
			response: answer,
			error: 'Nemotron did not include a command heading or the ## DONE marker.',
			processNumber: currentProcessIndex + 1,
			processCount: processIds.length,
		})
		return
	}

	currentCommand = next.command
	updateState({
		phase: 'approval',
		response: answer,
		processNumber: currentProcessIndex + 1,
		processCount: processIds.length,
		command: next.command,
	})
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
	const heading = answer.match(/^##\s+(.+?)\s*$/im)?.[1]?.trim()
	if (!heading) return { kind: 'missing' }
	if (/^(done|no more commands?)\b/i.test(heading)) return { kind: 'done' }

	const command = heading.replace(/^`|`$/g, '').trim()
	if (!command || /^<.*>$/.test(command)) return { kind: 'missing' }
	return { kind: 'command', command }
}

function requestSocket<T>(event: string, responseEvent: string, payload?: unknown): Promise<T> {
	return new Promise((resolve, reject) => {
		const timeout = window.setTimeout(() => {
			socket.off(responseEvent, onResponse)
			reject(new Error(`Timed out waiting for the ${responseEvent} Socket.IO message.`))
		}, 30_000)

		const onResponse = (response: T) => {
			window.clearTimeout(timeout)
			resolve(response)
		}

		socket.once(responseEvent, onResponse)
		if (payload === undefined) socket.emit(event)
		else socket.emit(event, payload)
	})
}

function requestBackup(
	event: 'create_backup' | 'restore_backup',
	payload?: { target: 'startup' | 'manual' },
): Promise<BackupResponse> {
	return new Promise((resolve, reject) => {
		const timeout = window.setTimeout(() => {
			reject(new Error(`Timed out waiting for the ${event} acknowledgement.`))
		}, 30_000)

		const acknowledge = (response: BackupResponse) => {
			window.clearTimeout(timeout)
			if (response?.success) {
				resolve(response)
				return
			}
			reject(new Error(response?.stderr || response?.error || response?.message || `${event} failed.`))
		}
		if (payload) socket.emit(event, payload, acknowledge)
		else socket.emit(event, acknowledge)
	})
}

function updateState(state: WorkflowState): void {
	stateListener?.(state)
}
