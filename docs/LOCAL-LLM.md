# Using your own quantized model

The agent talks to models through one interface (`ibos-llm`). Anything that speaks the **OpenAI chat-completions protocol**
already works today with `--model compat:<name> --base-url <url>`; that is how the benchmark's scripted model is connected.

```bash
intelli-rust run "open example.com and read the heading" --model compat:my-model --base-url http://127.0.0.1:8080/v1 --headless
```

So the quickest path is to put your model behind a local OpenAI-compatible server (llama.cpp's server, vLLM, Ollama, or your own
thin wrapper) and point `--base-url` at it. Nothing leaves the machine.

## What I need to go further (not built yet)
The custom quantized CPU/GPU module has not been added to this repository, and I have not seen it. To wire it in as a native
backend instead of an HTTP server I need to know:

1. Language and build system (Rust crate, C/C++ library, Python package?)
2. Its interface: how a prompt goes in and tokens come out; streaming or not; maximum context length
3. Quantization format (GGUF, GPTQ, AWQ, custom) and the CPU and GPU targets you support
4. Whether it supports tool/JSON-constrained output, which helps small models emit valid actions
5. A model small enough to run in CI, or a recorded transcript for tests

## Planned
- A `LocalBackend` implementing the same trait as the hosted clients, loaded in-process (no HTTP hop)
- Constrained decoding for the agent's action format
- Prompt variants for small models, chosen by measurement
- A benchmark of your model against hosted models on the same tasks (success, tokens, time)

Claims about small-model quality will only be made after that benchmark exists.
