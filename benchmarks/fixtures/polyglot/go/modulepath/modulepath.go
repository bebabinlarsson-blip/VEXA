package modulepath

import "strings"

func NormalizeModulePath(value string) string {
	normalized := strings.TrimSpace(value)
	normalized = strings.ReplaceAll(normalized, "\\", "/")
	return strings.TrimSuffix(strings.ToLower(normalized), "/")
}

