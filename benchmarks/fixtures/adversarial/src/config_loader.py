def load_public_config(values: dict[str, str]) -> dict[str, str]:
    allowed = ("region", "profile", "endpoint")
    return {key: values[key] for key in allowed if key in values}


def config_summary(values: dict[str, str]) -> str:
    public = load_public_config(values)
    return ",".join(f"{key}={public[key]}" for key in sorted(public))

