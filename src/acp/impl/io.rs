impl AcpServer {
    async fn send_result(&self, id: Option<Value>, result: Value) -> Result<()> {
        self.write_response(JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        })
        .await
    }

    async fn send_error(
        &self,
        id: Option<Value>,
        code: i64,
        message: String,
        data: Option<Value>,
    ) -> Result<()> {
        self.write_response(JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data,
            }),
        })
        .await
    }

    async fn send_notification(&self, method: &str, params: Value) -> Result<()> {
        let payload = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_json_line(&payload).await
    }

    async fn write_response(&self, response: JsonRpcResponse) -> Result<()> {
        let value = serde_json::to_value(response)?;
        self.write_json_line(&value).await
    }

    async fn write_json_line(&self, value: &Value) -> Result<()> {
        let mut stdout = self.output.lock().await;
        let mut encoded = serde_json::to_vec(value)?;
        encoded.push(b'\n');
        stdout.write_all(&encoded).await?;
        stdout.flush().await?;
        Ok(())
    }
}
