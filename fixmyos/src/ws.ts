import agentInstructions from '../../agent_instructions.md?raw'
import { Nemotron } from './nemotron'

const API_BASE_URL = 'http://127.0.0.1:8032'
const nemotron = new Nemotron()

export async function getErrors(): Promise<unknown> {
	return get(`${API_BASE_URL}/errors`)
}

export async function getError(id: string | number): Promise<unknown> {
	return get(`${API_BASE_URL}/errors/${encodeURIComponent(String(id))}`)
}

async function get(url: string): Promise<unknown> {
	const response = await fetch(url, { method: 'GET' })

	if (!response.ok) {
		throw new Error(`GET ${url} failed with HTTP ${response.status}`)
	}

	const data: unknown = await response.json()
	await handleResponse(data)
	return data
}

export async function handleResponse(response: unknown): Promise<string> {
	const question = `${agentInstructions}\n\n${JSON.stringify(response)}`
    let answer = await nemotron.ask(question)
    console.log(answer)
	return answer
}
