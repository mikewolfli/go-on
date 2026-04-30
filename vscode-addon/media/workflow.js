(function () {
  const vscode = acquireVsCodeApi();

  const workflowList = document.getElementById("workflowList");
  const createWorkflowBtn = document.getElementById("createWorkflowBtn");

  let workflows = {};

  function renderWorkflowList() {
    workflowList.innerHTML = "";
    Object.values(workflows).forEach((workflow) => {
      const item = document.createElement("div");
      item.className = "workflow-item";
      item.innerHTML = `<div><strong>${workflow.name}</strong> - ${workflow.status}</div>`;

      const controls = document.createElement("div");
      controls.className = "workflow-controls";

      const runBtn = document.createElement("button");
      runBtn.className = "workflow-btn";
      runBtn.textContent = "Run";
      runBtn.onclick = () =>
        vscode.postMessage({ type: "runWorkflow", workflowId: workflow.id });

      const deleteBtn = document.createElement("button");
      deleteBtn.className = "workflow-btn danger";
      deleteBtn.textContent = "Delete";
      deleteBtn.onclick = () => {
        vscode.postMessage({
          type: "showConfirm",
          message: "Delete this workflow?",
          id: "deleteWorkflow",
          workflowId: workflow.id,
        });
      };

      controls.appendChild(runBtn);
      controls.appendChild(deleteBtn);
      item.appendChild(controls);
      workflowList.appendChild(item);
    });
  }

  // Pending workflow-creation state for multi-step input flow
  let pendingWorkflowName = null;
  let pendingWorkflowSteps = [];
  let pendingWorkflowType = null;

  createWorkflowBtn.addEventListener("click", () => {
    // NOTE: vscode.window is NOT available in webview context.
    // We use postMessage to request the extension host to show UI elements.
    vscode.postMessage({
      type: "showInputBox",
      prompt: "Workflow name",
      id: "workflowName",
    });
  });

  window.addEventListener("message", (event) => {
    const message = event.data;
    switch (message.type) {
      case "workflowCreated":
        workflows[message.workflow.id] = message.workflow;
        renderWorkflowList();
        break;
      case "workflowStatusUpdate":
        if (workflows[message.workflowId]) {
          workflows[message.workflowId].status = message.status;
          renderWorkflowList();
        }
        break;
      case "showConfirmResult":
        if (message.id === "deleteWorkflow" && message.confirmed) {
          vscode.postMessage({
            type: "deleteWorkflow",
            workflowId: message.workflowId,
          });
        }
        break;
      case "workflowDeleted":
        delete workflows[message.workflowId];
        renderWorkflowList();
        break;
      case "showInputBoxResult":
        handleCreateFlowInput(message);
        break;
      case "showQuickPickResult":
        handleCreateFlowQuickPick(message);
        break;
    }
  });

  function handleCreateFlowInput(message) {
    if (message.id === "workflowName" && message.value) {
      pendingWorkflowName = message.value;
      // Ask for stage type
      vscode.postMessage({
        type: "showQuickPick",
        id: "workflowStageType",
        items: [
          { label: "Chat step", value: "chat" },
          { label: "Code step", value: "code" },
          { label: "Delay step", value: "delay" },
        ],
        placeHolder: "Add first stage type",
      });
    } else if (
      message.id === "workflowChatPrompt" &&
      message.value !== undefined
    ) {
      pendingWorkflowSteps.push({
        name: "Chat",
        type: "chat",
        prompt: message.value || "",
        status: "created",
      });
      finalizeWorkflowCreation();
    } else if (
      message.id === "workflowCodeSnippet" &&
      message.value !== undefined
    ) {
      pendingWorkflowSteps.push({
        name: "Code",
        type: "code",
        code: message.value || "",
        status: "created",
      });
      finalizeWorkflowCreation();
    } else if (message.id === "workflowDelaySeconds") {
      pendingWorkflowSteps.push({
        name: "Delay",
        type: "delay",
        delay: Number(message.value || "5"),
        status: "created",
      });
      finalizeWorkflowCreation();
    }
  }

  function handleCreateFlowQuickPick(message) {
    if (message.id === "workflowStageType" && message.value) {
      if (message.value === "chat") {
        vscode.postMessage({
          type: "showInputBox",
          prompt: "Prompt for chat step",
          id: "workflowChatPrompt",
        });
      } else if (message.value === "code") {
        vscode.postMessage({
          type: "showInputBox",
          prompt: "Code snippet (single line)",
          id: "workflowCodeSnippet",
        });
      } else if (message.value === "delay") {
        vscode.postMessage({
          type: "showInputBox",
          prompt: "Delay seconds",
          value: "5",
          id: "workflowDelaySeconds",
        });
      }
    }
  }

  function finalizeWorkflowCreation() {
    if (!pendingWorkflowName) return;
    vscode.postMessage({
      type: "createWorkflow",
      workflowData: {
        name: pendingWorkflowName,
        status: "created",
        steps: pendingWorkflowSteps,
      },
    });
    // Reset pending state
    pendingWorkflowName = null;
    pendingWorkflowSteps = [];
    pendingWorkflowType = null;
  }
})();
