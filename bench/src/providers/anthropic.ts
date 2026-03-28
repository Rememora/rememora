import Anthropic from "@anthropic-ai/sdk";
import type { Provider, CompletionResult, ToolCall } from "./types.js";
import { BASH_TOOL } from "./types.js";

export class AnthropicProvider implements Provider {
  name = "anthropic";
  private client: Anthropic;

  constructor() {
    this.client = new Anthropic();
  }

  async complete(
    model: string,
    systemPrompt: string,
    userMessage: string,
  ): Promise<CompletionResult> {
    const start = performance.now();

    const response = await this.client.messages.create({
      model,
      max_tokens: 1024,
      system: systemPrompt,
      tools: [
        {
          name: BASH_TOOL.name,
          description: BASH_TOOL.description,
          input_schema: {
            type: "object",
            properties: BASH_TOOL.parameters.properties,
            required: BASH_TOOL.parameters.required,
          },
        },
      ],
      messages: [{ role: "user", content: userMessage }],
    });

    const latencyMs = performance.now() - start;

    const toolCalls: ToolCall[] = response.content
      .filter((block): block is Anthropic.ToolUseBlock => block.type === "tool_use")
      .map((block) => ({
        id: block.id,
        name: block.name,
        input: block.input as Record<string, unknown>,
      }));

    return {
      model,
      provider: this.name,
      toolCalls,
      rawOutput: response,
      latencyMs,
      inputTokens: response.usage.input_tokens,
      outputTokens: response.usage.output_tokens,
    };
  }
}
