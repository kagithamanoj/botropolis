.PHONY: install install-dev test demo serve clean

install:
	pip install -e .

install-dev:
	pip install -e ".[dev]"

test:
	python -m pytest -q

demo:
	python examples/demo.py

serve:
	uvicorn botropolis.server:app --reload

clean:
	find . -name "__pycache__" -type d -prune -exec rm -rf {} +
	rm -rf .pytest_cache
