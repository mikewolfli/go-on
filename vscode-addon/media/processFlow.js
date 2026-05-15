(function () {
  // eslint-disable-next-line no-undef
  const vscode = acquireVsCodeApi();

  const processListEl = document.getElementById("processList");
  const processCanvas = document.getElementById("processCanvas");
  const createProcessBtn = document.getElementById("createProcessBtn");
  const runProcessBtn = document.getElementById("runProcessBtn");
  const exportProcessBtn = document.getElementById("exportProcessBtn");
  const importProcessBtn = document.getElementById("importProcessBtn");
  const importFileInput = document.getElementById("importFile");
  const currentTitle = document.getElementById("currentProcessTitle");

  let processes = {};
  let selectedProcessId = null;
  let draggingNode = null;
  let offsetX = 0;
  let offsetY = 0;

  function renderProcessList() {
    processListEl.innerHTML = "";

    Object.values(processes).forEach((process) => {
      const item = document.createElement("div");
      item.className = `process-item ${process.id === selectedProcessId ? "active" : ""}`;
      item.textContent = `${process.name || process.id} (${process.status || "idle"})`;
      item.onclick = () => selectProcess(process.id);
      processListEl.appendChild(item);
    });

    if (!selectedProcessId && Object.keys(processes).length > 0) {
      selectProcess(Object.keys(processes)[0]);
    }
  }

  function selectProcess(processId) {
    selectedProcessId = processId;
    renderProcessList();
    renderProcessFlow();
  }

  function renderProcessFlow() {
    processCanvas.innerHTML = "";

    if (!selectedProcessId || !processes[selectedProcessId]) {
      currentTitle.textContent = "No Process Selected";
      return;
    }

    const process = processes[selectedProcessId];
    currentTitle.textContent = `Process: ${process.name || process.id}`;

    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.style.position = "absolute";
    svg.style.top = "0";
    svg.style.left = "0";
    svg.style.width = "100%";
    svg.style.height = "100%";
    processCanvas.appendChild(svg);

    const nodes = (process.stages || []).map((stage, index) => {
      const node = document.createElement("div");
      node.className = `stage-node ${stage.status || ""}`;
      node.dataset.stageIndex = String(index);
      node.innerHTML = `<div class="stage-name">${stage.name || `Stage ${index + 1}`}</div><div class="stage-type">${stage.type || "step"}</div>`;

      node.onmousedown = (e) => {
        draggingNode = node;
        offsetX = e.offsetX;
        offsetY = e.offsetY;
      };

      processCanvas.appendChild(node);
      return node;
    });

    applyAutoLayout(nodes);

    function applyAutoLayout(nodes) {
      const width = processCanvas.clientWidth;
      const height = processCanvas.clientHeight;
      const cols = Math.max(1, Math.ceil(Math.sqrt(nodes.length)));
      const rows = Math.max(1, Math.ceil(nodes.length / cols));
      const hGap = Math.max(120, (width - 80) / cols);
      const vGap = Math.max(100, (height - 80) / rows);

      nodes.forEach((node, i) => {
        const row = Math.floor(i / cols);
        const col = i % cols;
        const targetX = 20 + col * hGap;
        const targetY = 20 + row * vGap;
        node.style.left = `${targetX}px`;
        node.style.top = `${targetY}px`;
      });

      // force-directed refinement step
      for (let iter = 0; iter < 20; iter++) {
        const forces = nodes.map(() => ({ x: 0, y: 0 }));

        nodes.forEach((n1, i1) => {
          const r1 = n1.getBoundingClientRect();
          nodes.forEach((n2, i2) => {
            if (i1 === i2) return;
            const r2 = n2.getBoundingClientRect();
            const dx = r1.left - r2.left;
            const dy = r1.top - r2.top;
            let dist = Math.sqrt(dx * dx + dy * dy);
            if (dist < 1) dist = 1;
            const repulsion = 1000 / (dist * dist);
            forces[i1].x += (dx / dist) * repulsion;
            forces[i1].y += (dy / dist) * repulsion;
          });
        });

        nodes.forEach((node, i) => {
          const f = forces[i];
          const x = parseFloat(node.style.left || "0") + f.x * 0.05;
          const y = parseFloat(node.style.top || "0") + f.y * 0.05;
          node.style.left = `${Math.max(0, Math.min(width - 140, x))}px`;
          node.style.top = `${Math.max(0, Math.min(height - 80, y))}px`;
        });
      }

      drawConnections(svg, process);
    }

    processCanvas.onmousemove = (e) => {
      if (!draggingNode) return;
      const rect = processCanvas.getBoundingClientRect();
      const x = e.clientX - rect.left - offsetX;
      const y = e.clientY - rect.top - offsetY;
      draggingNode.style.left = `${x}px`;
      draggingNode.style.top = `${y}px`;
      drawConnections(svg, process);
    };

    processCanvas.onmouseup = () => {
      draggingNode = null;
    };
    processCanvas.onmouseleave = () => {
      draggingNode = null;
    };

    drawConnections(svg);
  }

  function drawConnections(svg) {
    svg.innerHTML = "";

    const nodes = Array.from(processCanvas.querySelectorAll(".stage-node"));
    for (let i = 0; i < nodes.length - 1; i++) {
      const from = nodes[i].getBoundingClientRect();
      const to = nodes[i + 1].getBoundingClientRect();
      const container = processCanvas.getBoundingClientRect();

      const x1 = from.left + from.width;
      const y1 = from.top + from.height / 2;
      const x2 = to.left;
      const y2 = to.top + to.height / 2;

      const l = document.createElementNS("http://www.w3.org/2000/svg", "line");
      l.setAttribute("x1", String(x1 - container.left));
      l.setAttribute("y1", String(y1 - container.top));
      l.setAttribute("x2", String(x2 - container.left));
      l.setAttribute("y2", String(y2 - container.top));
      l.setAttribute("class", "connection-line");
      svg.appendChild(l);
    }
  }

  createProcessBtn.addEventListener("click", () => {
    // NOTE: vscode.window is NOT available in webview context.
    // We use postMessage to request the extension host to show UI elements.
    vscode.postMessage({
      type: "showInputBox",
      prompt: "Enter process name",
      id: "createProcessName",
    });
  });

  // Pending create-process state for multi-step input flow
  let pendingCreateName = null;

  runProcessBtn.addEventListener("click", () => {
    if (!selectedProcessId) {
      // NOTE: vscode.window is NOT available in webview context.
      vscode.postMessage({
        type: "showWarningMessage",
        message: "Select a process first",
      });
      return;
    }
    vscode.postMessage({ type: "runProcess", processId: selectedProcessId });
  });

  exportProcessBtn.addEventListener("click", () => {
    const text = JSON.stringify(processes, null, 2);
    const blob = new Blob([text], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `go-on-processes-${new Date().toISOString().replace(/[:.]/g, "-")}.json`;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  });

  importProcessBtn.addEventListener("click", () => {
    if (!importFileInput) return;
    importFileInput.value = "";
    importFileInput.click();
  });

  importFileInput.addEventListener("change", (event) => {
    const file = event.target.files[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = () => {
      try {
        const imported = JSON.parse(reader.result);
        vscode.postMessage({ type: "importProcesses", processes: imported });
      } catch (err) {
        // NOTE: vscode.window is NOT available in webview context.
        vscode.postMessage({
          type: "showErrorMessage",
          message: "Invalid JSON file",
        });
      }
    };
    reader.readAsText(file);
  });

  window.addEventListener("message", (event) => {
    const message = event.data;
    switch (message.type) {
      case "processesLoaded":
        processes = message.processes || {};
        renderProcessList();
        break;
      case "processCreated":
        processes[message.process.id] = message.process;
        selectedProcessId = message.process.id;
        renderProcessList();
        renderProcessFlow();
        break;
      case "processStatusUpdate":
        if (processes[message.processId]) {
          processes[message.processId].status = message.status;
          renderProcessList();
          renderProcessFlow();
        }
        break;
      case "stageStatusUpdate":
        if (processes[message.processId]) {
          const process = processes[message.processId];
          process.stages = process.stages || [];
          process.stages[message.stageIndex] =
            process.stages[message.stageIndex] || {};
          process.stages[message.stageIndex].status = message.status;
          renderProcessList();
          renderProcessFlow();
        }
        break;
      case "showInputBoxResult":
        // Handle multi-step create-process flow
        if (message.id === "createProcessName" && message.value) {
          pendingCreateName = message.value;
          vscode.postMessage({
            type: "showInputBox",
            prompt: "Stage count (default 3)",
            id: "createProcessStageCount",
            value: "3",
          });
        } else if (message.id === "createProcessStageCount") {
          const name = pendingCreateName;
          pendingCreateName = null;
          if (!name) return;
          const count = Number(message.value || "3") || 3;
          const stages = Array.from({ length: count }, (_, i) => ({
            id: `s_${i + 1}`,
            name: `Stage ${i + 1}`,
            type: "chat",
            status: "created",
          }));
          vscode.postMessage({
            type: "createProcess",
            processData: { name, stages, status: "created" },
          });
        }
        break;
    }
  });
})();
