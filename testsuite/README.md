# Kessel Test Suite

Capability tests for the kessel-cli CLI across multiple LLM backends, modeled
after `../klein-cli`'s testsuite (`runner.sh` + `matrix_runner.sh` + per-testcase
`prompt.txt`/`check.sh`).

Because the kessel-cli CLI takes a YAML `--config` and reads prompts from
**stdin** (a REPL, one line per turn) — and its tool set is read-only
(`read`/`glob`/`tasks`, no file writing) — the tests validate the assistant's
**text responses** rather than generated files.

## Layout

```
testsuite/
├── runner.sh            # run one testcase × one backend
├── matrix_runner.sh     # run all (filterable) → PASS/FAIL matrix
├── extract_response.sh  # pull assistant text (optionally per-turn) from output
├── backends/            # one YAML config per model
│   ├── gemma4.yaml       # local Gemma 4 E4B (llama.cpp)
│   ├── gpt-oss.yaml      # local GPT-OSS 20B (llama.cpp, harmony)
│   └── gpt-5.4-mini.yaml # cloud OpenAI (needs OPENAI_API_KEY)
├── testcases/
│   ├── arithmetic/       # 17 × 23 = 391
│   ├── capital/          # capital of France = Paris
│   ├── file_read/        # use the `read` tool on codeword.txt
│   ├── instruction/      # output exactly one given word
│   └── memory/           # 2-turn: recall a fact from turn 1
└── results/             # timestamped matrix logs (gitignored)
```

## Usage

```bash
# Build the CLI first (cloud-only, or scripts/build-win-local.bat for local models)
dotnet build win/KesselCli/KesselCli.csproj -c Release

# List testcases / backends
bash testsuite/runner.sh

# One testcase × one backend
bash testsuite/runner.sh capital gemma4

# Full matrix (all testcases × all available backends)
bash testsuite/matrix_runner.sh

# Filter (comma-separated)
BACKENDS="gemma4,gpt-oss"  bash testsuite/matrix_runner.sh
TESTS="memory,file_read"   bash testsuite/matrix_runner.sh
```

- `CLI` env var overrides the binary path (defaults to the Release build).
- `OPENAI_API_KEY` is read from the environment or a project-root `.env`
  (gitignored). Cloud backends are auto-skipped when no key is available.
- Each test runs in an isolated temp dir (its cwd), so the `read`/`glob` tools
  only see the testcase's own fixtures. Failed runs leave the temp dir for
  debugging; passed runs clean up.

## Adding a testcase

1. `mkdir testsuite/testcases/my_test`
2. `prompt.txt` — one user turn per non-empty line (`#` lines are comments)
3. `check.sh` (executable) — args `$1`=output file, `$2`=error file; cwd is the
   temp dir, with `./extract_response.sh` available. Exit 0 = pass.
4. Add any fixture files the test needs (copied into the temp workdir).

## Latest results (2026-06-17)

| backend       | arithmetic | capital | file_read | instruction | memory |
|---------------|:----------:|:-------:|:---------:|:-----------:|:------:|
| gemma4        | PASS | PASS | PASS | PASS | PASS |
| gpt-oss       | PASS | PASS | PASS | PASS | PASS |
| gpt-5.4-mini  | PASS | PASS | PASS | PASS | PASS |

All 15 combinations passed (100%).
