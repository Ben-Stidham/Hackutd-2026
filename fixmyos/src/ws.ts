import agentInstructions from '../../agent_instructions.md?raw'
import { io } from 'socket.io-client'
import { Nemotron } from './nemotron'

const nemotron = new Nemotron()
const socket = io(import.meta.env.VITE_SOCKET_URL || window.location.origin, {
	path: '/socket.io',
	transports: ['websocket'],
})

socket.on('connect_error', (error) => {
	console.error('Socket.IO connection failed:', error)
})

export async function getErrors(): Promise<unknown> {
	return request('get_errors')
}

export async function getError(id: string | number): Promise<unknown> {
	return request('get_error', { id })
}

async function request(event: string, payload?: unknown): Promise<unknown> {
	const requestWithTimeout = socket.timeout(10_000)
	const response: unknown =
		payload === undefined
			? await requestWithTimeout.emitWithAck(event)
			: await requestWithTimeout.emitWithAck(event, payload)
	await handleResponse(response)
	return response
}

export async function handleResponse(response: unknown): Promise<string> {
	const question = `${agentInstructions}\n\n${JSON.stringify(response)}`
	const answer = await nemotron.ask(question)
	console.log(answer)
	return answer
}
