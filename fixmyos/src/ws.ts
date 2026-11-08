import agentInstructions from '../../agent_instructions.md?raw'
import { io } from 'socket.io-client'
import { Nemotron } from './nemotron'

const nemotron = new Nemotron()

const socket = io(import.meta.env.VITE_SOCKET_URL || window.location.origin, {
	path: '/socket.io',
})

socket.on('connect_error', (error) => {
	console.error('Socket.IO connection failed:', error)
})

export async function getErrors(): Promise<string> {
	return request('get_errors')
}

export async function getError(id: string | number): Promise<string> {
	return request('get_error', { id })
}

function request(event: 'get_errors' | 'get_error', payload?: unknown): Promise<string> {
	const responseEvent = event === 'get_errors' ? 'errors_list' : 'error_details'

	return new Promise((resolve, reject) => {
		const timeout = window.setTimeout(() => {
			socket.off(responseEvent, onResponse)
			reject(new Error(`Timed out waiting for the ${responseEvent} Socket.IO message.`))
		}, 10_000)

		const onResponse = (response: unknown) => {
			window.clearTimeout(timeout)
			resolve(handleResponse(response))
		}

		socket.once(responseEvent, onResponse)
		if (payload === undefined) {
			socket.emit(event)
		} else {
			socket.emit(event, payload)
		}
	})
}

export async function handleResponse(response: unknown): Promise<string> {
	const question = `${agentInstructions}\n\n${JSON.stringify(response, null, 2)}`
	return nemotron.ask(question)
}
