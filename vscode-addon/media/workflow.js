(function() {
    const vscode = acquireVsCodeApi();

    const workflowList = document.getElementById('workflowList');
    const createWorkflowBtn = document.getElementById('createWorkflowBtn');

    let workflows = {};

    function renderWorkflowList() {
        workflowList.innerHTML = '';
        Object.values(workflows).forEach(workflow => {
            const item = document.createElement('div');
            item.className = 'workflow-item';
            item.innerHTML = `<div><strong>${workflow.name}</strong> - ${workflow.status}</div>`;

            const controls = document.createElement('div');
            controls.className = 'workflow-controls';

            const runBtn = document.createElement('button');
            runBtn.className = 'workflow-btn';
            runBtn.textContent = 'Run';
            runBtn.onclick = () => vscode.postMessage({ type: 'runWorkflow', workflowId: workflow.id });

            const deleteBtn = document.createElement('button');
            deleteBtn.className = 'workflow-btn danger';
            deleteBtn.textContent = 'Delete';
            deleteBtn.onclick = () => {
                if (confirm('Delete this workflow?')) {
                    vscode.postMessage({ type: 'deleteWorkflow', workflowId: workflow.id });
                }
            };

            controls.appendChild(runBtn);
            controls.appendChild(deleteBtn);
            item.appendChild(controls);
            workflowList.appendChild(item);
        });
    }

    createWorkflowBtn.addEventListener('click', async () => {
        const name = await vscode.window.showInputBox({ prompt: 'Workflow name' });
        if (!name) return;

        const action = await vscode.window.showQuickPick([
            { label: 'Chat step', value: 'chat' },
            { label: 'Code step', value: 'code' },
            { label: 'Delay step', value: 'delay' }
        ], { placeHolder: 'Add first stage type' });

        if (!action) return;

        let steps = [];

        if (action.value === 'chat') {
            const prompt = await vscode.window.showInputBox({ prompt: 'Prompt for chat step' });
            steps.push({ name: 'Chat', type: 'chat', prompt: prompt || '', status: 'created' });
        } else if (action.value === 'code') {
            const code = await vscode.window.showInputBox({ prompt: 'Code snippet (single line)' });
            steps.push({ name: 'Code', type: 'code', code: code || '', status: 'created' });
        } else {
            const delay = await vscode.window.showInputBox({ prompt: 'Delay seconds', value: '5' });
            steps.push({ name: 'Delay', type: 'delay', delay: Number(delay || '5'), status: 'created' });
        }

        vscode.postMessage({ type: 'createWorkflow', workflowData: { name, status: 'created', steps } });
    });

    window.addEventListener('message', event => {
        const message = event.data;
        switch (message.type) {
            case 'workflowCreated':
                workflows[message.workflow.id] = message.workflow;
                renderWorkflowList();
                break;
            case 'workflowStatusUpdate':
                if (workflows[message.workflowId]) {
                    workflows[message.workflowId].status = message.status;
                    renderWorkflowList();
                }
                break;
            case 'workflowDeleted':
                delete workflows[message.workflowId];
                renderWorkflowList();
                break;
        }
    });
})();
