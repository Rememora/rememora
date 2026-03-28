import { GoogleGenerativeAI } from "@google/generative-ai";
import type { Provider, CompletionResult, ToolCall } from "./types.js";
import { BASH_TOOL } from "./types.js";

export class GoogleProvider implements Provider {
  name = "google";
  private client: GoogleGenerativeAI;

  constructor() {
    const apiKey = process.env.GOOGLE_API_KEY ?? process.env.GEMINI_API_KEY;
    if (!apiKey) {
      throw new Error(
        "GOOGLE_API_KEY or GEMINI_API_KEY environment variable is required",
      );
    }
    this.client = new GoogleGenerativeAI(apiKey);
  }

  async complete(
    model: string,
    systemPrompt: string,
    userMessage: string,
  ): Promise<CompletionResult> {
    const start = performance.now();

    const genModel = this.client.getGenerativeModel({
      model,
      systemInstruction: systemPrompt,
      tools: [
        {
          functionDeclarations: [
            {
              name: BASH_TOOL.name,
              description: BASH_TOOL.description,
              parameters: {
                type: "OBJECT" as const,
                properties: {
                  command: {
                    type: "STRING" as const,
                    description: "The bash command to execute",
                  },
                },
                required: ["command"],
              },
            },
          ],
        },
      ],
    });

    const result = await genModel.generateContent(userMessage);
    const latencyMs = performance.now() - start;

    const response = result.response;
    const toolCalls: ToolCall[] = [];

    for (const candidate of response.candidates ?? []) {
      for (const part of candidate.content?.parts ?? []) {
        if (part.functionCall) {
          toolCalls.push({
            id: `google-${Date.now()}-${toolCalls.length}`,
            name: part.functionCall.name,
            input: (part.functionCall.args ?? {}) as Record<string, unknown>,
          });
        }
      }
    }

    const usage = response.usageMetadata;

    return {
      model,
      provider: this.name,
      toolCalls,
      rawOutput: response,
      latencyMs,
      inputTokens: usage?.promptTokenCount,
      outputTokens: usage?.candidatesTokenCount,
    };
  }
}
