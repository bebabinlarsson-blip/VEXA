export function formatLegacyTraceId(prefix, sequence) {
  const normalizedPrefix = String(prefix).trim().toLowerCase();
  return `${normalizedPrefix}-${String(sequence).padStart(6, "0")}`;
}

export function isLegacyTraceId(value) {
  return /^[a-z0-9_-]+-\d{6}$/.test(value);
}

