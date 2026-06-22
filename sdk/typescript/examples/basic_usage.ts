import { GoOnClient } from "../src/client";

async function main() {
  const client = new GoOnClient({ baseUrl: "http://localhost:8090" });

  // Check runtime health
  const health = await client.health();
  console.log("Health:", JSON.stringify(health, null, 2));

  // Stream a chat
  const stream = client.chatStream({
    messages: [{ role: "user", content: "Tell me a story" }],
  });
  for await (const chunk of stream) {
    console.log("Chunk:", JSON.stringify(chunk));
  }
  console.log("Stream finished");
}

main().catch(console.error);
