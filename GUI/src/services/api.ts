import axios from "axios";

export function buildClient(baseURL = "http://127.0.0.1:8090") {
    return axios.create({
        baseURL,
        timeout: 4000,
    });
}
