# Integrating go-on with Zed and VSCode

## 1. Prerequisites
- go-on backend service built
- Zed or VSCode editor installed

## 2. Start go-on Service
```bash
./target/release/go-on --acp-http-bind 127.0.0.1:8080
```

## 3. VSCode Integration
1. Recommended: install REST Client extension, or use Thunder Client/Postman.
2. Create a `.http` file, e.g.:
   ```http
   POST http://127.0.0.1:8080/chat
   Content-Type: application/json

   {
     "messages": [{"role": "user", "content": "Hello"}]
   }
   ```
3. Send the request to interact with the backend.
4. You can also develop a custom VSCode extension to call `/chat` directly.

## 4. Zed Integration
1. Zed supports plugins or terminal HTTP calls.
2. Use curl to send a request:
   ```bash
   curl -X POST http://127.0.0.1:8080/chat -H 'Content-Type: application/json' -d '{"messages":[{"role":"user","content":"Hello"}]}'
   ```
3. You can also develop a Zed plugin to integrate a chat window and call `/chat`.

## 5. Advanced
- Deploy go-on as a system service, or build a frontend for a full chat experience.
- See source code and API docs for further development.
