import * as assert from "node:assert";
import {
  parseSseDataLine,
  parseSseChunk,
  extractSseContent,
} from "../../runtime/sseStream";

suite("sseParsing", () => {
  suite("parseSseDataLine", () => {
    test("parses a valid data line", () => {
      const result = parseSseDataLine('data: {"token":"hello"}');
      assert.ok(result, "should return a parsed object");
      assert.strictEqual(result!.token, "hello");
    });

    test("parses a data line with content field", () => {
      const result = parseSseDataLine('data: {"content":"world"}');
      assert.ok(result);
      assert.strictEqual(result!.content, "world");
    });

    test("returns null for [DONE] marker", () => {
      const result = parseSseDataLine("data: [DONE]");
      assert.strictEqual(result, null);
    });

    test("returns null for empty line", () => {
      const result = parseSseDataLine("");
      assert.strictEqual(result, null);
    });

    test("returns null for whitespace-only line", () => {
      const result = parseSseDataLine("   ");
      assert.strictEqual(result, null);
    });

    test("returns null for line without data: prefix", () => {
      const result = parseSseDataLine("event: chunk");
      assert.strictEqual(result, null);
    });

    test("returns null for invalid JSON in data field", () => {
      const result = parseSseDataLine("data: {invalid}");
      assert.strictEqual(result, null);
    });

    test("handles data with extra whitespace after prefix", () => {
      const result = parseSseDataLine('data:  {"key":"value"}');
      assert.ok(result);
      assert.strictEqual(result!.key, "value");
    });

    test("handles data with leading whitespace", () => {
      const result = parseSseDataLine('  data: {"key":"value"}');
      assert.ok(result);
      assert.strictEqual(result!.key, "value");
    });

    test("parses nested JSON in data field", () => {
      const result = parseSseDataLine(
        'data: {"response":{"type":"text","content":"hello"}}',
      );
      assert.ok(result);
      assert.deepStrictEqual(result!.response, {
        type: "text",
        content: "hello",
      });
    });

    test("parses data with numeric fields", () => {
      const result = parseSseDataLine('data: {"index":0,"count":42}');
      assert.ok(result);
      assert.strictEqual(result!.index, 0);
      assert.strictEqual(result!.count, 42);
    });
  });

  suite("parseSseChunk", () => {
    test("parses a single data line", () => {
      const results = parseSseChunk('data: {"token":"hello"}\n');
      assert.strictEqual(results.length, 1);
      assert.strictEqual(results[0].token, "hello");
    });

    test("parses multiple data lines", () => {
      const chunk =
        'data: {"token":"hello"}\ndata: {"token":"world"}\ndata: [DONE]\n';
      const results = parseSseChunk(chunk);
      assert.strictEqual(results.length, 2);
      assert.strictEqual(results[0].token, "hello");
      assert.strictEqual(results[1].token, "world");
    });

    test("ignores event lines", () => {
      const chunk =
        'event: chunk\ndata: {"token":"hello"}\n\nevent: done\ndata: {"response":"ok"}\n';
      const results = parseSseChunk(chunk);
      assert.strictEqual(results.length, 2);
      assert.strictEqual(results[0].token, "hello");
      assert.strictEqual(results[1].response, "ok");
    });

    test("handles empty chunk", () => {
      const results = parseSseChunk("");
      assert.strictEqual(results.length, 0);
    });

    test("handles chunk with only non-data lines", () => {
      const results = parseSseChunk("event: done\n\n");
      assert.strictEqual(results.length, 0);
    });

    test("parses a complete SSE stream with multiple tokens", () => {
      const stream =
        'data: {"token":"The"}\ndata: {"token":" quick"}\ndata: {"token":" brown"}\ndata: {"token":" fox"}\ndata: [DONE]\n';
      const results = parseSseChunk(stream);
      assert.strictEqual(results.length, 4);
      const tokens = results.map((r) => r.token as string);
      assert.deepStrictEqual(tokens, ["The", " quick", " brown", " fox"]);
    });

    test("parses stream with content field tokens", () => {
      const stream = 'data: {"content":"Hello"}\ndata: {"content":" World"}\n';
      const results = parseSseChunk(stream);
      assert.strictEqual(results.length, 2);
      assert.strictEqual(results[0].content, "Hello");
      assert.strictEqual(results[1].content, " World");
    });
  });

  suite("extractSseContent", () => {
    test("extracts content field from data", () => {
      const content = extractSseContent({ content: "hello" });
      assert.strictEqual(content, "hello");
    });

    test("returns undefined when content field is missing", () => {
      const content = extractSseContent({ token: "hello" });
      assert.strictEqual(content, undefined);
    });

    test("returns undefined when content is not a string", () => {
      const content = extractSseContent({ content: 42 });
      assert.strictEqual(content, undefined);
    });

    test("returns undefined for empty object", () => {
      const content = extractSseContent({});
      assert.strictEqual(content, undefined);
    });

    test("extracts empty string content", () => {
      const content = extractSseContent({ content: "" });
      assert.strictEqual(content, "");
    });
  });
});
