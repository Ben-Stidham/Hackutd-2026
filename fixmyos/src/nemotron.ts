
const API_URL = "http://localhost:8080/v1/chat/completions";

type Message = {
    role: "system" | "user" | "assistant";
    content: string;
};

class Nemotron {
    private history: Message[] = [
        {
            role: "system",
            content: "You are a helpful assistant."
        }
    ];

    async ask(question: string): Promise<string> {
        this.history.push({
            role: "user",
            content: question
        });

        try {
            const response = await fetch(API_URL, {
                method: "POST",
                headers: {
                    "Content-Type": "application/json"
                },
                body: JSON.stringify({
                    model: "nemotron",
                    messages: this.history,
                    temperature: 1.0,
                    max_tokens: 1024
                })
            });

            if (!response.ok) {
                throw new Error(
                    `HTTP ${response.status}: ${await response.text()}`
                );
            }

            const data = await response.json();

            const answer: string =
                data.choices[0].message.content;

            this.history.push({
                role: "assistant",
                content: answer
            });

            return answer;

        } catch (error) {
            // Remove the unanswered user message so history stays consistent.
            this.history.pop();
            throw error;
        }
    }

    reset() {
        this.history = [
            {
                role: "system",
                content: "You are a helpful assistant."
            }
        ];
    }
}

// Example usage
async function main() {
    const bot = new Nemotron();

    console.log("Asking Nemotron...\n");

    console.log(await bot.ask("My name is Alex."));
    console.log(await bot.ask("What is my name?"));
    console.log(await bot.ask("Explain hash tables."));
}

main().catch(console.error);
