import { fetch } from "undici";

const BASE_URL = (process.env.SMARTSTUDIO_URL ?? "http://localhost:3001").replace(/\/$/, "");

export class HttpError extends Error {
  constructor(public status: number, public body: unknown, message: string) {
    super(message);
    this.name = "HttpError";
  }
}

async function request<T>(path: string, init: { method: string; body?: unknown }): Promise<T> {
  const url = `${BASE_URL}${path.startsWith("/") ? path : `/${path}`}`;
  const res = await fetch(url, {
    method: init.method,
    headers: { "content-type": "application/json", "accept": "application/json" },
    body: init.body === undefined ? undefined : JSON.stringify(init.body),
  });

  const text = await res.text();
  let parsed: unknown = text;
  if (text.length > 0) {
    try {
      parsed = JSON.parse(text);
    } catch {
      // leave as text
    }
  }

  if (!res.ok) {
    const msg =
      (typeof parsed === "object" && parsed && "error" in parsed && typeof (parsed as { error: unknown }).error === "string")
        ? (parsed as { error: string }).error
        : `${res.status} ${res.statusText}`;
    throw new HttpError(res.status, parsed, `${init.method} ${path} → ${res.status}: ${msg}`);
  }

  return parsed as T;
}

export const http = {
  get: <T>(path: string) => request<T>(path, { method: "GET" }),
  post: <T>(path: string, body?: unknown) => request<T>(path, { method: "POST", body: body ?? {} }),
  put: <T>(path: string, body?: unknown) => request<T>(path, { method: "PUT", body: body ?? {} }),
  del: <T>(path: string) => request<T>(path, { method: "DELETE" }),
  baseUrl: BASE_URL,
};
