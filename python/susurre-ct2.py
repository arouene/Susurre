#!/usr/bin/env python3
"""Transcribe a WAV file with faster-whisper.

Usage: susurre-ct2.py MODEL_DIR WAV LANG [PROMPT]
"""
import sys

from faster_whisper import WhisperModel


def main() -> int:
    model_dir, wav, lang = sys.argv[1:4]
    prompt = sys.argv[4] if len(sys.argv) > 4 else ""
    model = WhisperModel(model_dir, device="cpu", compute_type="int8")
    segments, _ = model.transcribe(
        wav,
        language=None if lang == "auto" else lang,
        # Silero VAD, shipped with faster-whisper: cuts silence before
        # decoding, which removes most hallucinations.
        vad_filter=True,
        vad_parameters={"min_silence_duration_ms": 300},
        beam_size=5,
        temperature=0.0,
        condition_on_previous_text=False,
        initial_prompt=prompt or None,
    )
    sys.stdout.write("".join(s.text for s in segments).strip())
    return 0


if __name__ == "__main__":
    sys.exit(main())
