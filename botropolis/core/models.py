"""Model client abstraction for Botropolis.

Supported providers: openai, anthropic, google (Gemini), ollama (local),
and stub (deterministic offline fallback).

Configuration comes from environment variables:

    OPENAI_API_KEY      OpenAI API key
    ANTHROPIC_API_KEY   Anthropic API key
    GOOGLE_API_KEY      Google AI Studio API key (Gemini)
    OLLAMA_HOST         Ollama base URL, default http://localhost:11434

When no key is configured for the requested model, the client falls back
to a deterministic offline stub. The stub never claims to be a real model
call: every stub response is clearly labeled as offline output.
"""

from __future__ import annotations

import hashlib
import os
from pathlib import Path
from typing import Dict, List, Optional

import requests
import yaml

PROVIDERS = ("openai", "anthropic", "google", "ollama", "stub")

_ENV_KEYS = {
    "openai": "OPENAI_API_KEY",
    "anthropic": "ANTHROPIC_API_KEY",
    "google": "GOOGLE_API_KEY",
    "ollama": "OLLAMA_HOST",
}

_DEFAULT_OLLAMA_HOST = "http://localhost:11434"


class ModelError(RuntimeError):
    """Raised when a provider call fails."""


def repo_root() -> Path:
    """Return the repository root (parent of the botropolis package)."""
    return Path(__file__).resolve().parents[2]


def load_model_registry(path: Optional[Path] = None) -> Dict:
    """Load models/registry.yaml, returning {} when it is missing."""
    registry_path = Path(path) if path else repo_root() / "models" / "registry.yaml"
    if not registry_path.exists():
        return {}
    with open(registry_path, "r", encoding="utf-8") as fh:
        return yaml.safe_load(fh) or {}


def find_model_entry(model_id: str, registry: Optional[Dict] = None) -> Optional[Dict]:
    """Find a model entry by id across base_models and fine_tunes."""
    registry = registry if registry is not None else load_model_registry()
    for section in ("base_models", "fine_tunes"):
        for entry in registry.get(section, []) or []:
            if isinstance(entry, dict) and entry.get("id") == model_id:
                return entry
    return None


def _provider_has_credentials(provider: str) -> bool:
    """Check whether the environment can serve this provider."""
    if provider == "stub":
        return True
    if provider == "ollama":
        return True  # local server; reachability is checked at call time
    key = _ENV_KEYS.get(provider)
    return bool(key and os.environ.get(key))


def _infer_provider(model_id: str) -> str:
    """Guess a provider from the model id when the registry has no entry.

    Unknown ids default to the stub: offline-safe, and every stub response
    is labeled so the fallback is never silent.
    """
    lowered = model_id.lower()
    if lowered == "stub":
        return "stub"
    if lowered.startswith("gpt") or lowered.startswith("o1") or lowered.startswith("o3"):
        return "openai"
    if lowered.startswith("claude"):
        return "anthropic"
    if lowered.startswith("gemini"):
        return "google"
    return "stub"


class ModelClient:
    """Route chat requests to a provider, or to the offline stub.

    Example:
        client = ModelClient()
        text = client.chat("gpt-4o", [{"role": "user", "content": "Hello"}])
    """

    def __init__(self, default_model: str = "stub") -> None:
        self.default_model = default_model
        self._registry = load_model_registry()

    def resolve_provider(self, model_id: str) -> str:
        """Pick the best provider for a model id, preferring stub offline."""
        entry = find_model_entry(model_id, self._registry)
        if entry:
            provider = entry.get("provider", "stub")
            if _provider_has_credentials(provider):
                return provider
            return "stub"
        provider = _infer_provider(model_id)
        if _provider_has_credentials(provider):
            return provider
        return "stub"

    def chat(self, model_id: Optional[str], messages: List[Dict[str, str]]) -> str:
        """Send chat messages and return the assistant text.

        Falls back to the deterministic offline stub when no credentials
        are configured for the model's provider.
        """
        model_id = model_id or self.default_model
        provider = self.resolve_provider(model_id)
        if provider == "stub":
            return stub_chat(model_id, messages)
        try:
            if provider == "openai":
                return _chat_openai(model_id, messages)
            if provider == "anthropic":
                return _chat_anthropic(model_id, messages)
            if provider == "google":
                return _chat_google(model_id, messages)
            if provider == "ollama":
                return _chat_ollama(model_id, messages)
        except requests.RequestException as exc:
            raise ModelError(f"{provider} request failed: {exc}") from exc
        raise ModelError(f"Unknown provider: {provider}")

    def provider_for(self, model_id: str) -> str:
        """Public helper used by agents to record which provider served a call."""
        return self.resolve_provider(model_id or self.default_model)


def _chat_openai(model_id: str, messages: List[Dict[str, str]]) -> str:
    """Call the OpenAI chat completions endpoint with requests."""
    key = os.environ["OPENAI_API_KEY"]
    resp = requests.post(
        "https://api.openai.com/v1/chat/completions",
        headers={"Authorization": f"Bearer {key}"},
        json={"model": model_id, "messages": messages},
        timeout=60,
    )
    resp.raise_for_status()
    return resp.json()["choices"][0]["message"]["content"]


def _chat_anthropic(model_id: str, messages: List[Dict[str, str]]) -> str:
    """Call the Anthropic messages endpoint with requests."""
    key = os.environ["ANTHROPIC_API_KEY"]
    system = "\n".join(m["content"] for m in messages if m["role"] == "system")
    convo = [m for m in messages if m["role"] != "system"]
    payload = {
        "model": model_id,
        "max_tokens": 1024,
        "messages": convo,
    }
    if system:
        payload["system"] = system
    resp = requests.post(
        "https://api.anthropic.com/v1/messages",
        headers={
            "x-api-key": key,
            "anthropic-version": "2023-06-01",
            "content-type": "application/json",
        },
        json=payload,
        timeout=60,
    )
    resp.raise_for_status()
    blocks = resp.json().get("content", [])
    return "".join(b.get("text", "") for b in blocks if b.get("type") == "text")


def _chat_google(model_id: str, messages: List[Dict[str, str]]) -> str:
    """Call the Gemini generateContent endpoint with requests."""
    key = os.environ["GOOGLE_API_KEY"]
    system = "\n".join(m["content"] for m in messages if m["role"] == "system")
    convo = [m for m in messages if m["role"] != "system"]
    contents = [
        {"role": "user" if m["role"] == "user" else "model",
         "parts": [{"text": m["content"]}]}
        for m in convo
    ]
    payload: Dict = {"contents": contents}
    if system:
        payload["systemInstruction"] = {"parts": [{"text": system}]}
    resp = requests.post(
        f"https://generativelanguage.googleapis.com/v1beta/models/{model_id}:generateContent",
        params={"key": key},
        json=payload,
        timeout=60,
    )
    resp.raise_for_status()
    data = resp.json()
    parts = data["candidates"][0]["content"].get("parts", [])
    return "".join(p.get("text", "") for p in parts)


def _chat_ollama(model_id: str, messages: List[Dict[str, str]]) -> str:
    """Call a local Ollama server with requests."""
    host = os.environ.get("OLLAMA_HOST", _DEFAULT_OLLAMA_HOST).rstrip("/")
    resp = requests.post(
        f"{host}/api/chat",
        json={"model": model_id, "messages": messages, "stream": False},
        timeout=120,
    )
    resp.raise_for_status()
    return resp.json()["message"]["content"]


def stub_chat(model_id: str, messages: List[Dict[str, str]]) -> str:
    """Deterministic offline response with structured reasoning.

    This is NOT a model call. It restates the task, shows the reasoning
    steps a specialist would take, and suggests next steps, so demos,
    tests, and the API server all work with no API keys and no network.
    """
    task = ""
    for message in reversed(messages):
        if message.get("role") == "user":
            task = message.get("content", "")
            break
    digest = hashlib.sha256(task.encode("utf-8")).hexdigest()
    variant = int(digest[:2], 16) % 3

    openers = [
        "Breaking this down methodically.",
        "Working through this step by step.",
        "Approaching this in a structured way.",
    ]
    words = task.split()
    focus = ", ".join(sorted({w.strip(".,!?").lower() for w in words if len(w) > 5})[:5])

    lines = [
        "[offline stub: no model API key configured, structured reasoning only]",
        "",
        f"Model requested: {model_id}",
        "",
        "Task restated:",
        task.strip(),
        "",
        f"Reasoning: {openers[variant]}",
        f"1. Identify the core ask and the key terms involved ({focus or 'general request'}).",
        "2. Apply domain expertise to separate facts, assumptions, and unknowns.",
        "3. Draft a response that is specific, checkable, and scoped to the request.",
        "4. Note what would need verification with live data or tools.",
        "",
        "Suggested next steps:",
        "- Refine the request with concrete constraints or examples.",
        "- Run the relevant builtin tools to ground the answer in data.",
        "- Escalate to a specialist agent if the task crosses departments.",
        "",
        "Set a model API key (OPENAI_API_KEY, ANTHROPIC_API_KEY, GOOGLE_API_KEY) "
        "or run Ollama locally to get a real model response instead of this stub.",
    ]
    return "\n".join(lines)
