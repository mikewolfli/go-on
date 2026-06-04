import { GoOnClient } from 'go-on-sdk';

async function main() {
  const client = new GoOnClient({ baseUrl: 'http://localhost:8090' });

  // Send a chat message
  const response = await client.chat({
    messages: [{ role: 'user', content: 'Hello!' }]
  });
  console.log('Response:', response);

  // Stream a chat
  const stream = client.chatStream({
    messages: [{ role: 'user', content: 'Tell me a story' }]
  });
  for await (const chunk of stream) {
    process.stdout.write(chunk);
  }
  console.log();

  await client.close();
}

main().catch(console.error);
