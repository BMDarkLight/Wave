export function formatInvokeError(err: unknown, fallback: string): string {
  if (err instanceof Error && err.message.trim()) return err.message;
  if (typeof err === "string" && err.trim()) return err;
  if (err && typeof err === "object") {
    const obj = err as Record<string, unknown>;
    for (const key of ["message", "error", "data"] as const) {
      const value = obj[key];
      if (typeof value === "string" && value.trim()) return value;
      if (value && typeof value === "object" && "message" in (value as object)) {
        const nested = (value as { message?: unknown }).message;
        if (typeof nested === "string" && nested.trim()) return nested;
      }
    }
  }
  return fallback;
}
