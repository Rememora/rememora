import OpenAI from "openai";
import type { Provider, CompletionResult, ToolCall } from "./types.js";
import { BASH_TOOL } from "./types.js";

export class OpenAIProvider implements Provider {
  name = "openai";
  private client: OpenAI;

  constructor() {
    this.client = new OpenAI();
  }

  async complete(
    model: string,
    systemPrompt: string,
    userMessage: string,
  ): Promise<CompletionResult> {
    const start = performance.now();

    const response = await this.client.chat.completions.create({
      model,
      max_tokens: 1024,
      messages: [
        { role: "system", content: systemPrompt },
        { role: "user", content: userMessage },
      ],
      tools: [
        {
          type: "function",
          function: {
            name: BASH_TOOL.name,
            description: BASH_TOOL.description,
            parameters: BASH_TOOL.parameters,
          },
        },
      ],
    });

    const latencyMs = performance.now() - start;

    const choice = response.choices[0];
    const toolCalls: ToolCall[] = (choice?.message?.tool_calls ?? []).map(
      (tc) => ({
        id: tc.id,
        name: tc.function.name,
        input: JSON.parse(tc.function.arguments) as Record<string, unknown>,
      }),
    );

    return {
      model,
      provider: this.name,
      toolCalls,
      rawOutput: response,
      latencyMs,
      inputTokens: response.usage?.prompt_tokens,
      outputTokens: response.usage?.completion_tokens,
    };
  }
}
