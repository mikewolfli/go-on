import protocolContract from "../assets/editor-capability-matrix.json";

export { protocolContract };

export const defaultRuntimeBaseUrl = protocolContract.runtime.baseUrl;
export const workflowControlModes = protocolContract.protocol
  .workflowControlModes ?? ["manual", "assisted", "autonomous"];
export const defaultWorkflowControlMode =
  protocolContract.protocol.defaultWorkflowControlMode ?? "assisted";
export const platformModes = protocolContract.protocol.platformModes ?? [
  "universal",
  "phase_compat",
];
export const defaultPlatformMode =
  protocolContract.protocol.defaultPlatformMode ?? "phase_compat";
