export function normalizeErrorMessage(
  error: unknown,
  fallback = "Unknown error",
): string {
  if (error instanceof Error) {
    return error.message || fallback;
  }
  if (typeof error === "string") {
    return error || fallback;
  }
  try {
    const serialized = JSON.stringify(error);
    return serialized && serialized !== "{}" ? serialized : fallback;
  } catch {
    return String(error ?? fallback);
  }
}

function prefixErrorMessage(
  prefix: string,
  error: unknown,
  fallback = "Unknown error",
): string {
  return `${prefix}${normalizeErrorMessage(error, fallback)}`;
}
