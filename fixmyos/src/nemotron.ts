const API_URL = '/nemotron/v1/chat/completions'

type Message = {
	role: 'system' | 'user' | 'assistant'
	content: string
}

const MAX_MODEL_LOADING_RETRIES = 8
const MAX_HISTORY_MESSAGES = 4
const MAX_HISTORY_MESSAGE_CHARS = 5000

export class Nemotron {
	private history: Message[] = [
		{
			role: 'system',
			content: 'You are a helpful assistant.',
		},
	]

	async ask(question: string): Promise<string> {
		const systemMessage = this.history[0] ?? {
			role: 'system' as const,
			content: 'You are a helpful assistant.',
		}
		const recentHistory = this.history
			.slice(1)
			.slice(-(MAX_HISTORY_MESSAGES - 1))
			.map((message) => ({
				...message,
				content: message.content.slice(0, MAX_HISTORY_MESSAGE_CHARS),
			}))
		this.history = [
			systemMessage,
			...recentHistory,
			{ role: 'user', content: question.slice(0, MAX_HISTORY_MESSAGE_CHARS) },
		]

		try {
			for (let attempt = 0; ; attempt++) {
				const response = await fetch(API_URL, {
					method: 'POST',
					headers: { 'Content-Type': 'application/json' },
					body: JSON.stringify({
						model: 'nemotron',
						messages: this.history,
						temperature: 1.0,
						max_tokens: 4096,
						chat_template_kwargs: { enable_thinking: false },
					}),
				})

				if (!response.ok) {
					const details = await response.text()
					if (response.status === 503 && /loading model/i.test(details)) {
						if (attempt < MAX_MODEL_LOADING_RETRIES) {
							const delay = Math.min(1000 * 2 ** attempt, 5000)
							await new Promise((resolve) => setTimeout(resolve, delay))
							continue
						}
						throw new Error('Nemotron is still loading the model. Please try again shortly.')
					}
					if (response.status === 502) {
						throw new Error(
							'Nemotron returned HTTP 502. The frontend proxy is reachable, but the service or SSH tunnel behind localhost:8080 is not responding.',
						)
					}
					throw new Error(`Nemotron HTTP ${response.status}: ${details || 'No response details.'}`)
				}

				const data = await response.json()
				const answer: string | undefined = data.choices?.[0]?.message?.content
				if (!answer?.trim()) {
					const finishReason = data.choices?.[0]?.finish_reason
					throw new Error(
						finishReason === 'length'
							? 'Nemotron used its response token limit before producing a visible answer.'
							: 'Nemotron returned an empty answer.',
					)
				}

				this.history.push({ role: 'assistant', content: answer })
				return answer
			}
		} catch (error) {
			this.history.pop()
			throw error
		}
	}

	reset(): void {
		this.history = [
			{
				role: 'system',
				content: 'You are a helpful assistant.',
			},
		]
	}
}

