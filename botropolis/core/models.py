"""Model client with an offline stub for local development."""


class ModelClient:
    def __init__(self, model: str = "offline-stub"):
        self.model = model

    def complete(self, system: str, prompt: str) -> str:
        if self.model == "offline-stub":
            return f"[offline stub] response to: {prompt[:80]}"
        raise NotImplementedError(f"provider for {self.model} not wired up yet")
