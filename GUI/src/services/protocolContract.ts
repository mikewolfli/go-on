import protocolContract from '../../../contracts/editor-capability-matrix.json';

export { protocolContract };

export const defaultRuntimeBaseUrl = protocolContract.runtime.baseUrl;
export const workflowControlModes =
	protocolContract.protocol.workflowControlModes ?? ["manual", "assisted", "autonomous"];
export const defaultWorkflowControlMode =
	protocolContract.protocol.defaultWorkflowControlMode ?? "assisted";
