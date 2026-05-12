// Settings functionality
(function () {
  const vscode = acquireVsCodeApi();
  const providerState = {
    providers: [],
    modelOptions: [],
    copilotAuth: {},
    secretTargets: [],
  };

  // Load settings when received
  window.addEventListener("message", (event) => {
    const message = event.data;

    // NOTE: "loadSettings" is not triggered by the backend (settings are sent as "settingsData" instead).
    // This handler is kept for forward-compatibility if the backend ever sends direct setting values.
    if (message.type === "loadSettings") {
      loadSettings(message.settings);
    } else if (message.type === "settingsData") {
      loadSettingsData(message.data || {});
    } else if (message.type === "providerModelsData") {
      providerState.copilotAuth = message.copilotAuth || providerState.copilotAuth;
      updateProviderModels(
        message.provider,
        Array.isArray(message.modelOptions) ? message.modelOptions : [],
        message.selectedModel || "auto",
        message.selectedEnvVar || "",
      );
    } else if (message.type === "copilotAuthState") {
      providerState.copilotAuth = message.auth || {};
      renderCopilotAuthState();
    } else if (message.type === "keyringResult") {
      renderKeyringOutput(message.message, message.value || "");
    } else if (message.type === "keyringError") {
      renderKeyringOutput(`Error: ${message.message}`, "");
    } else if (message.type === "settingsActionResult") {
      renderSettingsActionOutput(message.message);
    } else if (message.type === "settingsActionError") {
      renderSettingsActionOutput(`Error: ${message.message}`);
    } else if (message.type === "runtimeFeatures") {
      applyRuntimeFeatures(message.features || {});
    } else if (message.type === "focusCredentials") {
      // Scroll to the keyring/credentials section when navigating from API key prompt
      const keyringSection = document.querySelector(".setting-group h3");
      if (keyringSection && keyringSection.closest) {
        const container = keyringSection.closest(".setting-group");
        if (container) {
          container.scrollIntoView({ behavior: "smooth", block: "start" });
          // Highlight the secret value input briefly
          const secretInput = container.querySelector("#secretValue");
          if (secretInput) {
            secretInput.style.outline =
              "2px solid var(--vscode-inputOption-activeBorder)";
            setTimeout(() => {
              secretInput.style.outline = "";
            }, 2000);
          }
        }
      }
    }
  });

  function applyRuntimeFeatures(features) {
    document.querySelectorAll("[data-feature]").forEach((el) => {
      const required = String(el.getAttribute("data-feature") || "");
      const keys = required
        .split(",")
        .map((s) => s.trim())
        .filter(Boolean);
      const visible =
        keys.length === 0 || keys.some((k) => Boolean(features[k]));
      el.style.display = visible ? "" : "none";
    });
  }

  function loadSettings(settings) {
    Object.keys(settings).forEach((key) => {
      const elementId = key.replace("go-on.", "").replace(/\./g, "");
      const element = document.getElementById(elementId);

      if (element) {
        const value = settings[key];
        if (element.type === "checkbox") {
          element.checked = value;
        } else {
          element.value = value;
        }
      }
    });
  }

  function loadSettingsData(data) {
    if (!data || typeof data !== "object") {
      return;
    }

    const simpleFieldMap = {
      configPath: data.configPath,
      executablePath: data.executablePath,
      autoStart: data.autoStart,
    };

    Object.entries(simpleFieldMap).forEach(([id, value]) => {
      const element = document.getElementById(id);
      if (!element || value === undefined || value === null) {
        return;
      }
      if (element.type === "checkbox") {
        element.checked = Boolean(value);
      } else {
        element.value = String(value);
      }
    });

    const providerSettings = data.providerSettings || {};
    providerState.providers = Array.isArray(providerSettings.providers)
      ? providerSettings.providers
      : [];
    providerState.modelOptions = Array.isArray(providerSettings.modelOptions)
      ? providerSettings.modelOptions
      : [];
    providerState.copilotAuth = providerSettings.copilotAuth || {};
    providerState.secretTargets = Array.isArray(providerSettings.secretTargets)
      ? providerSettings.secretTargets
      : [];

    populateSecretSelect(providerState.secretTargets);

    populateProviderSelect(
      providerState.providers,
      providerSettings.selectedProvider || "",
    );
    populateModelSelect(
      providerState.modelOptions,
      providerSettings.selectedModel || "auto",
    );

    const envInput = document.getElementById("providerEnvVar");
    if (envInput && providerSettings.selectedEnvVar) {
      envInput.value = String(providerSettings.selectedEnvVar);
    }

    const clientIdInput = document.getElementById("copilotOauthClientId");
    if (clientIdInput) {
      clientIdInput.value = String(providerState.copilotAuth.oauthClientId || "");
    }

    toggleCopilotPanel(providerSettings.selectedProvider || "");
    renderCopilotAuthState();
  }

  function populateSecretSelect(secretTargets) {
    const select = document.getElementById("secretName");
    if (!select) {
      return;
    }

    const previousValue = select.value;
    select.innerHTML = "";

    secretTargets.forEach((target) => {
      const option = document.createElement("option");
      option.value = String(target.name || "");
      option.textContent = String(target.name || "");
      option.title = String(target.envVar || "");
      select.appendChild(option);
    });

    if (previousValue && secretTargets.some((item) => item.name === previousValue)) {
      select.value = previousValue;
    } else if (secretTargets.length > 0) {
      select.value = String(secretTargets[0].name || "");
    }
  }

  const GROUP_LABELS = {
    openai: "OpenAI Family",
    chinese: "Chinese Vendors",
    other: "Other Vendors",
  };

  function populateProviderSelect(providers, selectedProvider) {
    const select = document.getElementById("providerSelect");
    if (!select) {
      return;
    }

    select.innerHTML = "";

    // Group providers by region
    const groups = {};
    const order = ["openai", "chinese", "other"];
    providers.forEach((provider) => {
      const g = provider.group || "other";
      if (!groups[g]) groups[g] = [];
      groups[g].push(provider);
    });

    order.forEach((g) => {
      if (!groups[g] || groups[g].length === 0) return;
      const optgroup = document.createElement("optgroup");
      optgroup.label = GROUP_LABELS[g] || g;
      groups[g].forEach((provider) => {
        const option = document.createElement("option");
        option.value = provider.name;
        option.textContent = provider.name;
        optgroup.appendChild(option);
      });
      select.appendChild(optgroup);
    });

    if (
      selectedProvider &&
      providers.some((provider) => provider.name === selectedProvider)
    ) {
      select.value = selectedProvider;
    } else if (providers.length > 0) {
      select.value = providers[0].name;
    }
  }

  function populateModelSelect(modelOptions, selectedModel) {
    const select = document.getElementById("providerModelSelect");
    if (!select) {
      return;
    }

    const unique = [];
    const seen = new Set();
    modelOptions.forEach((item) => {
      const value = String(item || "").trim();
      if (!value || seen.has(value)) {
        return;
      }
      seen.add(value);
      unique.push(value);
    });
    if (!seen.has("auto")) {
      unique.unshift("auto");
    }

    select.innerHTML = "";
    unique.forEach((model) => {
      const option = document.createElement("option");
      option.value = model;
      option.textContent = model === "auto" ? "AUTO" : model;
      select.appendChild(option);
    });

    if (selectedModel && unique.includes(selectedModel)) {
      select.value = selectedModel;
    } else {
      select.value = "auto";
    }
  }

  function inferProviderEnvVar(provider) {
    return `${String(provider || "")
      .trim()
      .toUpperCase()
      .replace(/[-\s]+/g, "_")}_API_KEY`;
  }

  function updateProviderModels(
    provider,
    modelOptions,
    selectedModel,
    selectedEnvVar,
  ) {
    populateModelSelect(modelOptions, selectedModel);

    const envInput = document.getElementById("providerEnvVar");
    if (envInput) {
      if (selectedEnvVar) {
        envInput.value = selectedEnvVar;
      } else {
        const providerEntry = providerState.providers.find(
          (item) => item.name === provider,
        );
        envInput.value =
          providerEntry?.configuredEnvVar ||
          providerEntry?.apiKeyEnv ||
          inferProviderEnvVar(provider);
      }
    }

    toggleCopilotPanel(provider);
    renderCopilotAuthState();
  }

  function toggleCopilotPanel(provider) {
    const panel = document.getElementById("copilotAuthPanel");
    if (!panel) {
      return;
    }
    panel.style.display = provider === "copilot" ? "block" : "none";
  }

  function renderCopilotAuthState() {
    const output = document.getElementById("copilotAuthOutput");
    const cancelButton = document.getElementById("cancelCopilotDeviceFlow");
    const clientIdInput = document.getElementById("copilotOauthClientId");
    if (!output) {
      return;
    }

    const auth = providerState.copilotAuth || {};
    if (clientIdInput && auth.oauthClientId && !clientIdInput.value) {
      clientIdInput.value = auth.oauthClientId;
    }

    const lines = [];
    lines.push(`Authorized: ${auth.isAuthorized ? "yes" : "no"}`);
    lines.push(`Auth mode: ${auth.authMode || "none"}`);
    if (auth.accountLabel) {
      lines.push(`GitHub account: ${auth.accountLabel}`);
    }
    if (auth.userCode) {
      lines.push(`Device code: ${auth.userCode}`);
    }
    if (auth.verificationUri) {
      lines.push(`Verification URL: ${auth.verificationUri}`);
    }
    if (auth.modelSource) {
      lines.push(`Model source: ${auth.modelSource}`);
    }
    if (typeof auth.modelCount === "number") {
      lines.push(`Model count: ${auth.modelCount}`);
    }
    if (auth.statusMessage) {
      lines.push("");
      lines.push(auth.statusMessage);
    }
    if (auth.lastError) {
      lines.push("");
      lines.push(`Error: ${auth.lastError}`);
    }

    output.value = lines.join("\n");
    if (cancelButton) {
      cancelButton.disabled = !auth.pending;
    }
  }

  function updateSetting(key, value) {
    vscode.postMessage({
      type: "updateSetting",
      key,
      value,
    });
  }

  // Attach event listeners to all setting inputs
  document.querySelectorAll("[data-setting]").forEach((element) => {
    const settingKey = element.getAttribute("data-setting");

    if (element.type === "checkbox") {
      element.addEventListener("change", () => {
        updateSetting(settingKey, element.checked);
      });
    } else {
      element.addEventListener("change", () => {
        const value =
          element.type === "number" ? parseFloat(element.value) : element.value;
        updateSetting(settingKey, value);
      });
    }
  });

  // Action buttons
  function selectedSecretName() {
    return document.getElementById("secretName").value;
  }

  function secretValue() {
    return document.getElementById("secretValue").value;
  }

  function renderKeyringOutput(message, value) {
    const output = document.getElementById("keyringOutput");
    if (!output) return;

    output.value = value ? `${message}\n\n${value}` : message;
  }

  function renderSettingsActionOutput(message) {
    const output = document.getElementById("settingsActionOutput");
    if (!output) return;
    output.value = message;
  }

  function parseLineList(value) {
    return value
      .split("\n")
      .map((line) => line.trim())
      .filter(Boolean);
  }

  function parsePhaseRules(value) {
    const phaseRules = {};
    parseLineList(value).forEach((line) => {
      const parts = line.split("|");
      if (parts.length < 2) {
        return;
      }
      const phase = parts[0].trim();
      const rule = parts.slice(1).join("|").trim();
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

  const setKeyringSecretButton = document.getElementById("setKeyringSecret");
  if (setKeyringSecretButton) {
    setKeyringSecretButton.addEventListener("click", () => {
      const name = selectedSecretName();
      const value = secretValue();
      if (!value) {
        renderKeyringOutput("Error: Secret value cannot be empty.", "");
        return;
      }
      vscode.postMessage({
        type: "setKeyringSecret",
        name,
        value,
      });
    });
  }

  const getKeyringSecretButton = document.getElementById("getKeyringSecret");
  if (getKeyringSecretButton) {
    getKeyringSecretButton.addEventListener("click", () => {
      vscode.postMessage({
        type: "getKeyringSecret",
        name: selectedSecretName(),
      });
    });
  }

  const listKeyringSecretsButton =
    document.getElementById("listKeyringSecrets");
  if (listKeyringSecretsButton) {
    listKeyringSecretsButton.addEventListener("click", () => {
      vscode.postMessage({ type: "listKeyringSecrets" });
    });
  }

  const deleteKeyringSecretButton = document.getElementById(
    "deleteKeyringSecret",
  );
  if (deleteKeyringSecretButton) {
    deleteKeyringSecretButton.addEventListener("click", () => {
      vscode.postMessage({
        type: "deleteKeyringSecret",
        name: selectedSecretName(),
      });
    });
  }

  const applyDefaultTemplateButton = document.getElementById(
    "applyDefaultTemplate",
  );
  if (applyDefaultTemplateButton) {
    applyDefaultTemplateButton.addEventListener("click", () => {
      const template = document.getElementById("defaultTemplate").value;
      vscode.postMessage({
        type: "applyDefaultConfigTemplate",
        template,
      });
    });
  }

  const openConfigWizardButton = document.getElementById("openConfigWizard");
  if (openConfigWizardButton) {
    openConfigWizardButton.addEventListener("click", () => {
      vscode.postMessage({ type: "openConfigWizard" });
    });
  }

  const applyRulesSettingsButton =
    document.getElementById("applyRulesSettings");
  if (applyRulesSettingsButton) {
    applyRulesSettingsButton.addEventListener("click", () => {
      const globalRules = parseLineList(
        document.getElementById("globalRules").value,
      );
      const commonRules = parseLineList(
        document.getElementById("commonRules").value,
      );
      const phaseRules = parsePhaseRules(
        document.getElementById("phaseRules").value,
      );

      vscode.postMessage({
        type: "applyRulesSettings",
        payload: {
          globalRules,
          commonRules,
          phaseRules,
        },
      });
    });
  }

  const applyWorkflowMappingButton = document.getElementById(
    "applyWorkflowMapping",
  );
  if (applyWorkflowMappingButton) {
    applyWorkflowMappingButton.addEventListener("click", () => {
      const defaultPhase = document
        .getElementById("defaultPhaseInput")
        .value.trim();
      const raw = document.getElementById("workflowMapping").value.trim();
      if (!raw) {
        renderSettingsActionOutput("Error: Node mapping JSON cannot be empty.");
        return;
      }

      let phases;
      try {
        phases = JSON.parse(raw);
      } catch (error) {
        renderSettingsActionOutput(
          `Error: Invalid JSON - ${error.message || String(error)}`,
        );
        return;
      }

      vscode.postMessage({
        type: "applyWorkflowMapping",
        payload: {
          defaultPhase,
          phases,
        },
      });
    });
  }

  const providerSelect = document.getElementById("providerSelect");
  if (providerSelect) {
    providerSelect.addEventListener("change", () => {
      const provider = providerSelect.value;
      toggleCopilotPanel(provider);
      vscode.postMessage({ type: "requestProviderModels", provider });
    });
  }

  const authorizeCopilotGitHubSessionButton = document.getElementById(
    "authorizeCopilotGitHubSession",
  );
  if (authorizeCopilotGitHubSessionButton) {
    authorizeCopilotGitHubSessionButton.addEventListener("click", () => {
      vscode.postMessage({ type: "authorizeCopilotGitHubSession" });
    });
  }

  const authorizeCopilotDeviceFlowButton = document.getElementById(
    "authorizeCopilotDeviceFlow",
  );
  if (authorizeCopilotDeviceFlowButton) {
    authorizeCopilotDeviceFlowButton.addEventListener("click", () => {
      const oauthClientId =
        document.getElementById("copilotOauthClientId")?.value?.trim() || "";
      if (!oauthClientId) {
        renderSettingsActionOutput(
          "Error: GitHub OAuth client ID is required for device flow.",
        );
        return;
      }
      vscode.postMessage({
        type: "authorizeCopilotDeviceFlow",
        oauthClientId,
      });
    });
  }

  const refreshCopilotModelsButton = document.getElementById(
    "refreshCopilotModels",
  );
  if (refreshCopilotModelsButton) {
    refreshCopilotModelsButton.addEventListener("click", () => {
      vscode.postMessage({ type: "requestProviderModels", provider: "copilot" });
    });
  }

  const cancelCopilotDeviceFlowButton = document.getElementById(
    "cancelCopilotDeviceFlow",
  );
  if (cancelCopilotDeviceFlowButton) {
    cancelCopilotDeviceFlowButton.addEventListener("click", () => {
      vscode.postMessage({ type: "cancelCopilotDeviceFlow" });
    });
  }

  const deleteCopilotAuthorizationButton = document.getElementById(
    "deleteCopilotAuthorization",
  );
  if (deleteCopilotAuthorizationButton) {
    deleteCopilotAuthorizationButton.addEventListener("click", () => {
      vscode.postMessage({ type: "deleteCopilotAuthorization" });
    });
  }

  const applyProviderSelectionButton = document.getElementById(
    "applyProviderSelection",
  );
  if (applyProviderSelectionButton) {
    applyProviderSelectionButton.addEventListener("click", () => {
      const provider = document.getElementById("providerSelect")?.value || "";
      const model =
        document.getElementById("providerModelSelect")?.value || "auto";
      const envVar = document.getElementById("providerEnvVar")?.value || "";
      if (!provider) {
        renderSettingsActionOutput("Error: Please choose a provider first.");
        return;
      }
      vscode.postMessage({
        type: "saveProviderSelection",
        provider,
        model,
        envVar,
      });
    });
  }

  document.getElementById("startGoOn").addEventListener("click", () => {
    vscode.postMessage({ type: "startGoOn" });
  });

  document.getElementById("stopGoOn").addEventListener("click", () => {
    vscode.postMessage({ type: "stopGoOn" });
  });

  document.getElementById("healthCheck").addEventListener("click", () => {
    vscode.postMessage({ type: "healthCheck" });
  });

  document.getElementById("breakerStatus").addEventListener("click", () => {
    vscode.postMessage({ type: "breakerStatus" });
  });

  document.getElementById("breakerRecovery").addEventListener("click", () => {
    vscode.postMessage({ type: "breakerRecovery" });
  });

  document.getElementById("clearCache").addEventListener("click", () => {
    vscode.postMessage({ type: "clearCache" });
  });

  document.getElementById("clearVector").addEventListener("click", () => {
    vscode.postMessage({ type: "clearVector" });
  });

  document.getElementById("reloadConfig").addEventListener("click", () => {
    vscode.postMessage({ type: "reloadConfig" });
  });

  document.getElementById("workflowExecute").addEventListener("click", () => {
    vscode.postMessage({ type: "workflowExecute" });
  });

  document.getElementById("taskPlan").addEventListener("click", () => {
    vscode.postMessage({ type: "taskPlan" });
  });

  document.getElementById("taskExecute").addEventListener("click", () => {
    vscode.postMessage({ type: "taskExecute" });
  });

  document.getElementById("learningSummary").addEventListener("click", () => {
    vscode.postMessage({ type: "learningSummary" });
  });

  document.getElementById("learningGuardrail").addEventListener("click", () => {
    vscode.postMessage({ type: "learningGuardrail" });
  });

  document.getElementById("learningReplay").addEventListener("click", () => {
    vscode.postMessage({ type: "learningReplay" });
  });

  document.getElementById("knowledgeDistill").addEventListener("click", () => {
    vscode.postMessage({ type: "knowledgeDistill" });
  });

  document.getElementById("rlAlignmentEval").addEventListener("click", () => {
    vscode.postMessage({ type: "rlAlignmentEval" });
  });

  document.getElementById("hardnessStatus").addEventListener("click", () => {
    vscode.postMessage({ type: "hardnessStatus" });
  });

  document.getElementById("costStatus").addEventListener("click", () => {
    vscode.postMessage({ type: "costStatus" });
  });

  document.getElementById("configBaseline").addEventListener("click", () => {
    vscode.postMessage({ type: "configBaseline" });
  });

  document.getElementById("errorContract").addEventListener("click", () => {
    vscode.postMessage({ type: "errorContract" });
  });

  document.getElementById("buildRepro").addEventListener("click", () => {
    vscode.postMessage({ type: "buildRepro" });
  });

  document.getElementById("dataLifecycle").addEventListener("click", () => {
    vscode.postMessage({ type: "dataLifecycle" });
  });

  document.getElementById("optimizationPeak").addEventListener("click", () => {
    vscode.postMessage({ type: "optimizationPeak" });
  });

  document.getElementById("runtimeStability").addEventListener("click", () => {
    vscode.postMessage({ type: "runtimeStability" });
  });

  document.getElementById("autotuneStatus").addEventListener("click", () => {
    vscode.postMessage({ type: "autotuneStatus" });
  });

  document.getElementById("governancePlanGet").addEventListener("click", () => {
    vscode.postMessage({ type: "governancePlanGet" });
  });

  document
    .getElementById("governanceAuditRecent")
    .addEventListener("click", () => {
      vscode.postMessage({ type: "governanceAuditRecent" });
    });

  document.getElementById("healthProbes").addEventListener("click", () => {
    vscode.postMessage({ type: "healthProbes" });
  });

  document.getElementById("lockStatus").addEventListener("click", () => {
    vscode.postMessage({ type: "lockStatus" });
  });

  document
    .getElementById("observabilityAlerts")
    .addEventListener("click", () => {
      vscode.postMessage({ type: "observabilityAlerts" });
    });

  document.getElementById("securityBaseline").addEventListener("click", () => {
    vscode.postMessage({ type: "securityBaseline" });
  });

  document.getElementById("harnessStatus").addEventListener("click", () => {
    vscode.postMessage({ type: "harnessStatus" });
  });

  document.getElementById("governanceStatus").addEventListener("click", () => {
    vscode.postMessage({ type: "governanceStatus" });
  });

  // Request initial settings
  vscode.postMessage({ type: "requestSettings" });
})();
