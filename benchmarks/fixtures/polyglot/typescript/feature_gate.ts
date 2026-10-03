export type GateMap = Readonly<Record<string, boolean>>;

export function resolveFeatureGate(
  gates: GateMap,
  name: string,
  fallback: boolean,
): boolean {
  return Object.prototype.hasOwnProperty.call(gates, name) ? gates[name] : fallback;
}

