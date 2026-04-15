// Settings functionality
(function () {
    const vscode = acquireVsCodeApi();

    // Load settings when received
    window.addEventListener('message', event => {
        const message = event.data;

        if (message.type === 'loadSettings') {
            loadSettings(message.settings);
        } else if (message.type === 'keyringResult') {
            renderKeyringOutput(message.message, message.value || '');
        } else if (message.type === 'keyringError') {
            renderKeyringOutput(`Error: ${message.message}`, '');
        } else if (message.type === 'settingsActionResult') {
            renderSettingsActionOutput(message.message);
        } else if (message.type === 'settingsActionError') {
            renderSettingsActionOutput(`Error: ${message.message}`);
        }
    });

    function loadSettings(settings) {
        Object.keys(settings).forEach(key => {
            const elementId = key.replace('go-on.', '').replace(/\./g, '');
            const element = document.getElementById(elementId);

            if (element) {
                const value = settings[key];
                if (element.type === 'checkbox') {
                    element.checked = value;
                } else {
                    element.value = value;
                }
            }
        });
    }

    function updateSetting(key, value) {
        vscode.postMessage({
            type: 'updateSetting',
            key,
            value
        });
    }

    // Attach event listeners to all setting inputs
    document.querySelectorAll('[data-setting]').forEach(element => {
        const settingKey = element.getAttribute('data-setting');

        if (element.type === 'checkbox') {
            element.addEventListener('change', () => {
                updateSetting(settingKey, element.checked);
            });
        } else {
            element.addEventListener('change', () => {
                const value = element.type === 'number' ? parseFloat(element.value) : element.value;
                updateSetting(settingKey, value);
            });
        }
    });

    // Action buttons
    function selectedSecretName() {
        return document.getElementById('secretName').value;
    }

    function secretValue() {
        return document.getElementById('secretValue').value;
    }

    function renderKeyringOutput(message, value) {
        const output = document.getElementById('keyringOutput');
        if (!output) return;

        output.value = value ? `${message}\n\n${value}` : message;
    }

    function renderSettingsActionOutput(message) {
        const output = document.getElementById('settingsActionOutput');
        if (!output) return;
        output.value = message;
    }

    function parseLineList(value) {
        return value
            .split('\n')
            .map(line => line.trim())
            .filter(Boolean);
    }

    function parsePhaseRules(value) {
        const phaseRules = {};
        parseLineList(value).forEach(line => {
            const parts = line.split('|');
            if (parts.length < 2) {
                return;
            }
            const phase = parts[0].trim();
            const rule = parts.slice(1).join('|').trim();
            if (!phase || !rule) {
                return;
            }
            if (!phaseRules[phase]) {
                phaseRules[phase] = [];
            }
            phaseRules[phase].push(rule);
        });
        return phaseRules;
    }

    const setKeyringSecretButton = document.getElementById('setKeyringSecret');
    if (setKeyringSecretButton) {
        setKeyringSecretButton.addEventListener('click', () => {
            const name = selectedSecretName();
            const value = secretValue();
            if (!value) {
                renderKeyringOutput('Error: Secret value cannot be empty.', '');
                return;
            }
            vscode.postMessage({
                type: 'setKeyringSecret',
                name,
                value
            });
        });
    }

    const getKeyringSecretButton = document.getElementById('getKeyringSecret');
    if (getKeyringSecretButton) {
        getKeyringSecretButton.addEventListener('click', () => {
            vscode.postMessage({
                type: 'getKeyringSecret',
                name: selectedSecretName()
            });
        });
    }

    const listKeyringSecretsButton = document.getElementById('listKeyringSecrets');
    if (listKeyringSecretsButton) {
        listKeyringSecretsButton.addEventListener('click', () => {
            vscode.postMessage({ type: 'listKeyringSecrets' });
        });
    }

    const deleteKeyringSecretButton = document.getElementById('deleteKeyringSecret');
    if (deleteKeyringSecretButton) {
        deleteKeyringSecretButton.addEventListener('click', () => {
            vscode.postMessage({
                type: 'deleteKeyringSecret',
                name: selectedSecretName()
            });
        });
    }

    const applyDefaultTemplateButton = document.getElementById('applyDefaultTemplate');
    if (applyDefaultTemplateButton) {
        applyDefaultTemplateButton.addEventListener('click', () => {
            const template = document.getElementById('defaultTemplate').value;
            vscode.postMessage({
                type: 'applyDefaultConfigTemplate',
                template
            });
        });
    }

    const applyRulesSettingsButton = document.getElementById('applyRulesSettings');
    if (applyRulesSettingsButton) {
        applyRulesSettingsButton.addEventListener('click', () => {
            const globalRules = parseLineList(document.getElementById('globalRules').value);
            const commonRules = parseLineList(document.getElementById('commonRules').value);
            const phaseRules = parsePhaseRules(document.getElementById('phaseRules').value);

            vscode.postMessage({
                type: 'applyRulesSettings',
                payload: {
                    globalRules,
                    commonRules,
                    phaseRules
                }
            });
        });
    }

    const applyWorkflowMappingButton = document.getElementById('applyWorkflowMapping');
    if (applyWorkflowMappingButton) {
        applyWorkflowMappingButton.addEventListener('click', () => {
            const defaultPhase = document.getElementById('defaultPhaseInput').value.trim();
            const raw = document.getElementById('workflowMapping').value.trim();
            if (!raw) {
                renderSettingsActionOutput('Error: Node mapping JSON cannot be empty.');
                return;
            }

            let phases;
            try {
                phases = JSON.parse(raw);
            } catch (error) {
                renderSettingsActionOutput(`Error: Invalid JSON - ${error.message || String(error)}`);
                return;
            }

            vscode.postMessage({
                type: 'applyWorkflowMapping',
                payload: {
                    defaultPhase,
                    phases
                }
            });
        });
    }

    document.getElementById('startGoOn').addEventListener('click', () => {
        vscode.postMessage({ type: 'startGoOn' });
    });

    document.getElementById('stopGoOn').addEventListener('click', () => {
        vscode.postMessage({ type: 'stopGoOn' });
    });

    document.getElementById('healthCheck').addEventListener('click', () => {
        vscode.postMessage({ type: 'healthCheck' });
    });

    document.getElementById('breakerStatus').addEventListener('click', () => {
        vscode.postMessage({ type: 'breakerStatus' });
    });

    document.getElementById('breakerRecovery').addEventListener('click', () => {
        vscode.postMessage({ type: 'breakerRecovery' });
    });

    document.getElementById('clearCache').addEventListener('click', () => {
        vscode.postMessage({ type: 'clearCache' });
    });

    document.getElementById('clearVector').addEventListener('click', () => {
        vscode.postMessage({ type: 'clearVector' });
    });

    document.getElementById('reloadConfig').addEventListener('click', () => {
        vscode.postMessage({ type: 'reloadConfig' });
    });

    document.getElementById('workflowExecute').addEventListener('click', () => {
        vscode.postMessage({ type: 'workflowExecute' });
    });

    document.getElementById('taskPlan').addEventListener('click', () => {
        vscode.postMessage({ type: 'taskPlan' });
    });

    document.getElementById('taskExecute').addEventListener('click', () => {
        vscode.postMessage({ type: 'taskExecute' });
    });

    document.getElementById('learningSummary').addEventListener('click', () => {
        vscode.postMessage({ type: 'learningSummary' });
    });

    document.getElementById('learningGuardrail').addEventListener('click', () => {
        vscode.postMessage({ type: 'learningGuardrail' });
    });

    document.getElementById('learningReplay').addEventListener('click', () => {
        vscode.postMessage({ type: 'learningReplay' });
    });

    document.getElementById('knowledgeDistill').addEventListener('click', () => {
        vscode.postMessage({ type: 'knowledgeDistill' });
    });

    document.getElementById('rlAlignmentEval').addEventListener('click', () => {
        vscode.postMessage({ type: 'rlAlignmentEval' });
    });

    document.getElementById('hardnessStatus').addEventListener('click', () => {
        vscode.postMessage({ type: 'hardnessStatus' });
    });

    document.getElementById('costStatus').addEventListener('click', () => {
        vscode.postMessage({ type: 'costStatus' });
    });

    document.getElementById('configBaseline').addEventListener('click', () => {
        vscode.postMessage({ type: 'configBaseline' });
    });

    document.getElementById('errorContract').addEventListener('click', () => {
        vscode.postMessage({ type: 'errorContract' });
    });

    document.getElementById('buildRepro').addEventListener('click', () => {
        vscode.postMessage({ type: 'buildRepro' });
    });

    document.getElementById('dataLifecycle').addEventListener('click', () => {
        vscode.postMessage({ type: 'dataLifecycle' });
    });

    document.getElementById('optimizationPeak').addEventListener('click', () => {
        vscode.postMessage({ type: 'optimizationPeak' });
    });

    document.getElementById('runtimeStability').addEventListener('click', () => {
        vscode.postMessage({ type: 'runtimeStability' });
    });

    document.getElementById('autotuneStatus').addEventListener('click', () => {
        vscode.postMessage({ type: 'autotuneStatus' });
    });

    document.getElementById('governancePlanGet').addEventListener('click', () => {
        vscode.postMessage({ type: 'governancePlanGet' });
    });

    document.getElementById('governanceAuditRecent').addEventListener('click', () => {
        vscode.postMessage({ type: 'governanceAuditRecent' });
    });

    document.getElementById('healthProbes').addEventListener('click', () => {
        vscode.postMessage({ type: 'healthProbes' });
    });

    document.getElementById('observabilityAlerts').addEventListener('click', () => {
        vscode.postMessage({ type: 'observabilityAlerts' });
    });

    document.getElementById('securityBaseline').addEventListener('click', () => {
        vscode.postMessage({ type: 'securityBaseline' });
    });

    document.getElementById('harnessStatus').addEventListener('click', () => {
        vscode.postMessage({ type: 'harnessStatus' });
    });

    document.getElementById('governanceStatus').addEventListener('click', () => {
        vscode.postMessage({ type: 'governanceStatus' });
    });

    // Request initial settings
    vscode.postMessage({ type: 'requestSettings' });
})();