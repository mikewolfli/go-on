# Zed 与 VSCode 集成 go-on 教程

## 1. 前提条件
- 已编译好 go-on 后端服务
- 已安装 Zed 或 VSCode 编辑器

## 2. 启动 go-on 服务
```bash
./target/release/go-on --acp-http-bind 127.0.0.1:8080
```

## 3. VSCode 集成
1. 推荐安装 REST Client 插件，或使用 Thunder Client/Postman。
2. 新建 `.http` 文件，内容示例：
   ```http
   POST http://127.0.0.1:8080/chat
   Content-Type: application/json

   {
     "messages": [{"role": "user", "content": "你好"}]
   }
   ```
3. 发送请求即可与后端对话。
4. 也可开发自定义 VSCode 扩展，直接调用 `/chat` 接口。

## 4. Zed 集成
1. Zed 支持通过插件或终端调用 HTTP。
2. 可用 curl 发送请求：
   ```bash
   curl -X POST http://127.0.0.1:8080/chat -H 'Content-Type: application/json' -d '{"messages":[{"role":"user","content":"你好"}]}'
   ```
3. 也可开发 Zed 插件，集成对话窗口，调用 `/chat`。

## 5. 进阶
- 可将 go-on 服务部署为系统服务，或结合前端页面实现完整对话体验。
- 参考源码和接口文档进行二次开发。
