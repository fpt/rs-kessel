using System.Globalization;
using System.Runtime.Versioning;
using System.Speech.Synthesis;
using System.Text.RegularExpressions;

namespace VoiceAgentCLI;

/// Text-to-speech via the built-in Windows synthesizer (System.Speech).
/// Used by listen mode to speak the agent's replies.
[SupportedOSPlatform("windows")]
internal sealed class VoiceOutput : IDisposable
{
    private SpeechSynthesizer? _synth;

    public VoiceOutput(string culture)
    {
        try
        {
            var synth = new SpeechSynthesizer();
            var voices = synth.GetInstalledVoices().Where(v => v.Enabled).ToList();
            if (voices.Count == 0)
            {
                Console.Error.WriteLine("[warn] no TTS voices installed; replies won't be spoken.");
                synth.Dispose();
                return;
            }

            // Prefer a voice matching the language; otherwise use the first one.
            var want = new CultureInfo(culture);
            var pick = voices.FirstOrDefault(v =>
                           v.VoiceInfo.Culture.TwoLetterISOLanguageName == want.TwoLetterISOLanguageName)
                       ?? voices[0];
            synth.SelectVoice(pick.VoiceInfo.Name);
            synth.SetOutputToDefaultAudioDevice();
            _synth = synth;
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[warn] text-to-speech unavailable: {ex.Message}");
            _synth = null;
        }
    }

    /// Speak synchronously (blocks until done, so the mic isn't re-opened until
    /// the reply finishes — avoids the agent hearing itself). On any failure,
    /// TTS is disabled so we don't warn every turn.
    public void Speak(string text)
    {
        if (_synth is null) return;
        var clean = CleanForSpeech(text);
        if (clean.Length == 0) return;
        try
        {
            _synth.Speak(clean);
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[warn] TTS disabled after error: {ex.Message}");
            _synth.Dispose();
            _synth = null;
        }
    }

    /// Drop reasoning blocks and basic markdown so the synth reads the answer,
    /// not the chain-of-thought or formatting characters.
    private static string CleanForSpeech(string s)
    {
        s = Regex.Replace(s, "<think>.*?</think>", " ", RegexOptions.Singleline | RegexOptions.IgnoreCase);
        s = Regex.Replace(s, "[`*#_>]", " ");
        return s.Trim();
    }

    public void Dispose() => _synth?.Dispose();
}
