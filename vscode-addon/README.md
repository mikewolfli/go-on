# Go-On VS Code Extension

This VS Code extension provides an enhanced chat interface for the Go-On Rust ACP proxy, featuring improved chat capabilities, code execution, session management, and real-time status monitoring.

## ✨ Features

### 🤖 Enhanced Chat Interface
- **Interactive Chat Panel**: Dedicated side panel for conversations with Go-On
- **Markdown Rendering**: Rich text formatting with syntax highlighting
- **Code Execution**: Run JavaScript code blocks directly in chat
- **Session Management**: Create and switch between multiple chat sessions
- **Persistent Sessions**: Chat history survives VS Code restarts
- **Message History**: Persistent chat history within sessions

### 🛠️ Code Intelligence
- **JavaScript Execution**: Run JavaScript code blocks with real-time results
- **Python Execution**: Execute Python code with proper error handling and timeouts
- **Shell Command Execution**: Run Bash/cmd commands safely with output capture
- **Copy Code**: One-click copying of code blocks
- **Syntax Highlighting**: Proper code syntax highlighting in chat

### 📊 Real-Time Monitoring
- **Status Bar Integration**: Live Go-On proxy status indicators
- **Health Monitoring**: Automatic health checks with visual feedback
- **Connection Status**: Real-time connection state in Explorer panel

### 🧩 工作流与流程可视化
- **流程工作流面板**: 可创建/运行/删除工作流
- **多阶段流程视图**: 节点拖拽、连线、状态追踪、力导向自动布局
- **高阶布局**: 网格与拓扑优化（自动跨层排列、碰撞避免）
- **状态更新**: 运行中/成功/失败可见
- **命令面板支持**: `go-on.createWorkflow`, `go-on.runWorkflow`, `go-on.showProcessFlow`

### 🤖 AI高级编辑模板（更深）
- **智能模板**: 强化 prompt 语义 + 结构化结果要求
- **多维度约束**: 代码行为保留、性能、可读、安全、异常
- **输出选项**: 替换文本、新文档、剪贴板
- **可扩展**: 增加业务规则/语言风格/团队规范

### 🎨 Enhanced User Interface
- **Activity Bar Integration**: Dedicated Go-On section
- **Session Controls**: Create, switch, clear, and export chat sessions
- **Responsive Design**: Adapts to VS Code's theme and scaling
- **Session Persistence**: Chat sessions survive VS Code restarts

## 🚀 Quick Start

1. **Install Dependencies**:
   ```bash
   cd vscode-addon
   npm install
   npm run compile
   ```

2. **Launch Extension**:
   - Press `F5` in VS Code to start extension development host
   - Or use: `code --extensionDevelopmentPath=. --disable-extensions`

3. **Configure Go-On**:
   - Open Go-On Settings panel
   - Set config and executable paths
   - Start the Go-On proxy

4. **Start Chatting**:
   - Click the robot icon in the Activity Bar
   - Type your message and press Enter
   - Try code execution with JavaScript, Python, or shell code blocks

4. **Start Chatting**:
   - Click the robot icon in the Activity Bar
   - Type your message and press Enter
   - Try code execution with JavaScript blocks

## 📋 Interface Overview

### Activity Bar (Go-On Section)
- **Chat**: Enhanced conversation panel with markdown and code execution
- **Settings**: Basic configuration interface

### Explorer Panel
- **Go-On Status**: Real-time connection and health indicators

### Command Palette
Access all features via `Ctrl+Shift+P`:
- `Go-On: Start Go-On Proxy`
- `Go-On: Stop Go-On Proxy`
- `Go-On: Send Chat Request to Go-On`
- `Go-On: Check Go-On Health`
- `Go-On: Clear Cache`
- `Go-On: New Session`
- `Go-On: Switch Session`
- `Go-On: Open Go-On Chat`

### Chat Features
- **Markdown Support**: Rich text formatting in messages
- **Code Blocks**: Syntax highlighted code with copy/run buttons
- **Session Management**: Create and switch between chat sessions
- **Real-time Execution**: JavaScript code execution with instant results

## ⚙️ Configuration Options

### System Settings
- **Config Path**: Location of `config.toml`
- **Executable Path**: Go-On binary location
- **Auto-start**: Launch Go-On when opening workspace

### Chat Configuration
- **Max History**: Number of messages to retain
- **Streaming**: Real-time response streaming

### Monitoring
- **Health Interval**: Status check frequency
- **Status Bar**: Show connection status in VS Code status bar
- **Theme**: Auto/light/dark mode
- **Font Size**: Interface text size (8-24px)

## 🔧 Advanced Usage

### Chat Features
- **Regular Chat**: Type any message for conversation with Go-On
- **Code Execution**: Use JavaScript, Python, and shell code blocks for immediate execution
- **Multi-language Support**: JavaScript (eval), Python (subprocess), Shell (cmd/bash)
- **Execution Timeouts**: Automatic timeout protection (10s for Python, 15s for shell)
- **Error Handling**: Detailed error messages and exit codesimmediate execution
- **Session Management**: Create multiple sessions for different conversations
- **Session Controls**: UI buttons for new, switch, clear, and export sessions
- **Persistent Storage**: Sessions and messages saved across VS Code restarts
- **Markdown Support**: Rich formatting in chat messages

### Management Operations
- **Process Control**: Start/stop the Go-On proxy
- **Health Monitoring**: Regular status checks with visual indicators
- **Cache Management**: Clear accumulated cache data
- **Session Management**: Create and switch between chat sessions
runs in sandbox, Python/shell have timeouts and restrictions
- **Python Not Found**: Ensure Python is installed and in PATH
- **Shell Commands**: Limited to safe operations with timeout protection
### Troubleshooting
- **Connection Issues**: Check status panel and status bar
- **Code Execution**: JavaScript execution works, Python/Bash planned for future
- **Session Issues**: Use command palette for session management
- **Extension Logs**: Check VS Code Developer Console

## 🏗️ Development

### Building
```bash
npm run compile
```

### Testing
```bash
npm test
```

### Launching Development Host
```bash
code --extensionDevelopmentPath=. --disable-extensions
```

### Debugging
- Us� Security Considerations

### Code Execution
- **JavaScript**: Runs in a sandboxed environment with limited access
- **Python/Shell**: Execute in subprocess with timeouts and working directory restrictions
- **Timeout Protection**: Automatic termination after 10-15 seconds to prevent hanging
- **Working Directory**: Limited to extension directory for security
- **No Network Access**: Code execution is isolated from network operations

### Best Practices
- Only execute code from trusted sources
- Use code execution for learning and experimentation
- Avoid running untrusted shell commands
- Monitor resource usage during execution
### Current Architecturemulti-language code execution
- **StatusMonitor**: Real-time status monitoring and health checks
- **GoOnManager**: Process management and JSON-RPC communication
- **Session Management**: Multi-session support with persistent storage
- **Code Execution**: Sandboxed JavaScript, subprocess Python/shell with security measureson
- **Session Management**: Multi-session support with switching

## 📄 License

Same as the Go-On project.

## 🤝 Contributing

1. Follow the main Go-On project's development rules
2. Test thoroughly with different Go-On configurations
3. Ensure UI responsiveness and accessibility
4. Document new features and configuration options