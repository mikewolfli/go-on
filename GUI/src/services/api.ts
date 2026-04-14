import axios from "axios";

import { defaultRuntimeBaseUrl } from "./protocolContract";

export function buildClient(baseURL = defaultRuntimeBaseUrl) {
    return axios.create({
        baseURL,
        timeout: 4000,
    });
}
