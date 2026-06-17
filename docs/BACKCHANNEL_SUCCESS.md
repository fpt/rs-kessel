# Backchannel Response Implementation - Success Report

**Date**: 2025-10-15
**Status**: ✅ Complete
**Branch**: main

## Overview

Successfully implemented **backchannel responses** for natural voice conversation flow. The system now provides real-time acknowledgments ("mm-hmm", "got it", "uh-huh") during user speech without polluting the conversation history or degrading LLM context quality.

## Design Philosophy

### Core Principle: Separation of Concerns

**Experiential Layer (Audio)**:
- Backchannel responses ("mm-hmm", "got it") spoken immediately
- Provides natural conversational feedback
- Improves user experience and turn-taking

**Reasoning Layer (State)**:
- State Capsule (compact JSON) captures conversation intent/entities
- Only meaningful state updates added to context
- Keeps LLM context dense (100-200 tokens per capsule)
- Backchannel text NOT added to message history

### Architecture Diagram

```
User speaks "I mixed the eggs and..."
    ↓
[Pause detected: 500ms]
    ↓
STT: Quick transcribe partial audio
    ↓
RuleBasedStateUpdater: Extract intent/entities
    ↓
Should backchannel? → YES: "mm-hmm"
    ↓
┌─────────────────────────┬─────────────────────────┐
│  Audio Layer (TTS)      │  State Layer (Memory)   │
├─────────────────────────┼─────────────────────────┤
│ Speak "mm-hmm"          │ Update State Capsule:   │
│ (audio only)            │ - intent: "status_update"│
│ No history entry        │ - entities: {completion} │
│                         │ Add marker: "⟂"         │
└─────────────────────────┴─────────────────────────┘
    ↓
User continues speaking...
    ↓
[Full utterance complete]
    ↓
Main LLM call gets:
- State Capsule (compact context)
- Message history (no backchannel text)
- Dense, high-quality context
```

## Implementation

### 1. State Capsule (Rust)

**File**: `crates/agent-core/src/state_capsule.rs`

Compact representation of conversation state:

```rust
pub struct StateCapsule {
    /// Current intent (e.g., "cook_step_help", "timer_request")
    pub intent: String,

    /// Extracted entities (e.g., {"recipe": "omelet", "step": "2"})
    pub entities: HashMap<String, String>,

    /// User goals (e.g., ["finish step 2", "set timer"])
    pub user_goals: Vec<String>,

    /// Conversation tone ("neutral", "confused", "satisfied")
    pub tone: String,

    /// Open slots to fill (e.g., ["timer_minutes?"])
    pub open_slots: Vec<String>,

    /// Confidence score (0.0-1.0)
    pub confidence: f32,
}
```

**Key features**:
- Serializes to 100-200 tokens (strict limit)
- Updates incrementally during conversation
- Injected as system message to LLM
- Provides rich context without verbosity

### 2. Lightweight State Updater (Rust)

**File**: `crates/agent-core/src/state_updater.rs`

Rule-based state extraction and backchannel detection:

```rust
pub trait StateUpdater: Send + Sync {
    /// Update state based on user utterance
    fn update(&self, prev: &StateCapsule, utterance: &str) -> Result<StateCapsule>;

    /// Detect if backchannel should trigger
    fn should_backchannel(&self, utterance: &str, pause_ms: u64) -> Option<String>;
}
```

**RuleBasedStateUpdater** (current implementation):
- **Entity extraction**: Timer requests, step progression, completion status
- **Intent detection**: timer_request, step_help_request, status_update, clarification_request
- **Tone detection**: confused, satisfied, frustrated
- **Backchannel triggers**:
  - Short utterance (<10 words) + pause >500ms → "got it", "mm-hmm"
  - Conjunction words (and, but, so) + pause >400ms → "uh-huh"

**Future**: Can be replaced with small model (2-3B) for better accuracy.

### 3. Enhanced Conversation Memory (Rust)

**File**: `crates/agent-core/src/memory.rs`

Memory now tracks both messages and state:

```rust
pub struct ConversationMemory {
    messages: Vec<MessageEntry>,  // Regular messages
    pub state_capsule: StateCapsule,  // Compact state
}

// Backchannel marker "⟂" for tempo tracking only
pub fn add_backchannel(&mut self) {
    self.messages.push(MessageEntry {
        message: ChatMessage {
            role: ChatRole::Assistant,
            content: "⟂",  // Marker, not actual text
        },
        is_backchannel: true,
    });
}
```

**Key methods**:
- `add_message()` - Regular conversation message
- `add_backchannel()` - Tempo marker (excluded from LLM context)
- `get_messages()` - Returns messages WITHOUT backchannel markers
- `get_state_prompt()` - Formats State Capsule as prompt fragment
- `update_state_capsule()` - Updates compact state

### 4. Agent Integration (Rust)

**File**: `crates/agent-core/src/lib.rs`

Two processing paths:

```rust
impl Agent {
    /// Main conversation step (full LLM call)
    pub fn step(&self, user_input: String) -> Result<AgentResponse> {
        // 1. Update state capsule
        let updated_capsule = self.state_updater.update(&prev_capsule, &user_input)?;
        memory.update_state_capsule(updated_capsule);

        // 2. Add user message
        memory.add_message(ChatMessage { role: User, content: user_input });

        // 3. Prepend state capsule to messages
        let state_prompt = memory.get_state_prompt();
        messages.insert(0, ChatMessage { role: System, content: state_prompt });

        // 4. Call LLM
        let response = self.client.chat(&messages)?;

        // 5. Add assistant response
        memory.add_message(ChatMessage { role: Assistant, content: response });

        Ok(response)
    }

    /// Backchannel processing (lightweight, no LLM)
    pub fn process_backchannel(&self, partial_input: String, pause_ms: u64) -> Option<String> {
        // Check if should backchannel
        if let Some(backchannel_text) = self.state_updater.should_backchannel(&partial_input, pause_ms) {
            // Update state capsule
            let updated_capsule = self.state_updater.update(&prev_capsule, &partial_input)?;
            memory.update_state_capsule(updated_capsule);

            // Add tempo marker (not added to LLM context)
            memory.add_backchannel();

            return Some(backchannel_text);
        }
        None
    }
}
```

### 5. STT Backchannel Detection (Swift)

**File**: `swift/Sources/STT/SpeechToText.swift`

Enhanced audio processing with pause detection:

```swift
public func startRecording(
    onAutoStop: (() -> Void)? = nil,
    onBackchannel: ((_ partialText: String, _ pauseMs: UInt64) -> Void)? = nil
) async throws

private func processAudioBufferSync(_ buffer: AVAudioPCMBuffer) {
    // Voice activity detection
    if audioLevel > config.silenceThreshold {
        // Voice detected
        lastVoiceActivityTime = Date()
        // Start auto-stop timer...
    } else if let lastVoiceTime = lastVoiceActivityTime {
        // Check for backchannel opportunity
        let pauseDuration = Date().timeIntervalSince(lastVoiceTime)

        // Trigger on moderate pauses (400-700ms)
        if pauseDuration > 0.4 && pauseDuration < 0.7 {
            // Quick partial transcription
            if let partial = try? await quickTranscribe() {
                backchannelCallback?(partial, UInt64(pauseDuration * 1000))
            }
        }
    }
}

/// Quick transcription for backchannel (no silence trimming)
private func quickTranscribe() async throws -> String? {
    // Uses last 3 seconds of audio
    // Fast transcription without full processing
    // Returns partial text or nil
}
```

**Timing strategy**:
- **400-700ms pause**: Backchannel opportunity
  - Not too short (normal speech rhythm)
  - Not too long (already auto-stopping)
- **>1000ms pause**: Auto-stop triggers (end of utterance)

### 6. Voice Mode Integration (Swift)

**File**: `swift/Sources/KesselCli/main.swift`

Backchannel integrated into main voice loop:

```swift
func runVoiceMode() async {
    while turnCount < maxTurns {
        try await stt.startRecording(
            onAutoStop: {
                // Handle end of utterance
                let result = try await stt.stopRecording()
                continuation.resume(returning: result)
            },
            onBackchannel: { partialText, pauseMs in
                // Backchannel detection
                if let backchannelText = agent.processBackchannel(
                    partialInput: partialText,
                    pauseMs: pauseMs
                ) {
                    print("💬 ", terminator: "")  // Visual indicator

                    // Speak WITHOUT adding to history
                    await tts.speakAsync(backchannelText)
                }
            }
        )

        // Process full utterance through agent
        if let text = transcription {
            let response = try agent.step(userInput: text)
            await tts.speakAsync(response)
        }
    }
}
```

## Usage

### Build and Run

```bash
# 1. Build Rust with new state capsule
cd crates
cargo build --release

# 2. Regenerate UniFFI bindings
cd ..
bash scripts/gen_uniffi.sh

# 3. Build Swift
cd swift
swift build

# 4. Run voice mode (backchannel enabled)
make run
```

### Voice Mode Example

```
🎤 Listening...

User: "I mixed the eggs and..."
💬  [Agent speaks: "mm-hmm"]
User: "...added salt and pepper. What's next?"
✋ Silence detected, stopping...

Transcribed: I mixed the eggs and added salt and pepper. What's next?