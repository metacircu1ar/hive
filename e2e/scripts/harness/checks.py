"""Evidence checks that remain active under Python optimization."""


def require(condition: object, message: object = "Invalid scenario evidence") -> None:
    if not condition:
        raise RuntimeError(message)


def one(values, label: str, *, qualifier: str = "one", count_label: str = "got"):
    require(len(values) == 1, f"Expected {qualifier} {label}, {count_label} {len(values)}")
    return values[0]
