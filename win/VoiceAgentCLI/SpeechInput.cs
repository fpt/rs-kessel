using System.Globalization;
using System.Runtime.Versioning;
using System.Speech.Recognition;

namespace VoiceAgentCLI;

/// Outcome of a listen-mode capture.
internal enum ListenOutcome
{
    Recognized, // got a phrase (text is non-null)
    Toggled,    // user pressed Shift+Tab to leave listen mode
    Unavailable // no microphone / recognizer
}

/// Speech-to-text via the built-in Windows desktop recognizer (System.Speech).
[SupportedOSPlatform("windows")]
internal static class SpeechInput
{
    private static bool _warnedFallback;

    /// Build an engine for the requested BCP-47 culture (e.g. "en-US"). Falls
    /// back to a same-language recognizer, then the system default, warning once
    /// if the exact culture isn't installed.
    private static SpeechRecognitionEngine CreateEngine(string culture)
    {
        var want = new CultureInfo(culture);
        var installed = SpeechRecognitionEngine.InstalledRecognizers();
        var match =
            installed.FirstOrDefault(r => string.Equals(r.Culture.Name, want.Name, StringComparison.OrdinalIgnoreCase))
            ?? installed.FirstOrDefault(r => r.Culture.TwoLetterISOLanguageName == want.TwoLetterISOLanguageName);

        if (match is null && !_warnedFallback)
        {
            _warnedFallback = true;
            var have = installed.Count == 0 ? "(none)" : string.Join(", ", installed.Select(r => r.Culture.Name));
            Console.Error.WriteLine(
                $"[warn] no '{culture}' speech recognizer installed; using default. Installed: {have}. " +
                "Add one via Settings > Time & language > Speech.");
        }

        var engine = match is not null ? new SpeechRecognitionEngine(match) : new SpeechRecognitionEngine();
        engine.SetInputToDefaultAudioDevice();
        engine.LoadGrammar(new DictationGrammar());
        return engine;
    }

    /// Capture a single spoken utterance (blocking until speech or timeout).
    public static string? RecognizeOnce(string culture, int initialSilenceSeconds = 10)
    {
        try
        {
            using var engine = CreateEngine(culture);
            engine.InitialSilenceTimeout = TimeSpan.FromSeconds(initialSilenceSeconds);
            engine.BabbleTimeout = TimeSpan.FromSeconds(3);
            engine.EndSilenceTimeout = TimeSpan.FromMilliseconds(800);
            return engine.Recognize()?.Text;
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[warn] speech recognition unavailable: {ex.Message}");
            return null;
        }
    }

    /// Listen-mode capture: recognize one phrase while polling `toggleRequested`.
    /// Recognition runs async so Shift+Tab stays responsive; the mic is released
    /// on return (half-duplex — no self-hearing while the agent speaks).
    public static (ListenOutcome Outcome, string? Text) Listen(string culture, Func<bool> toggleRequested)
    {
        SpeechRecognitionEngine engine;
        try
        {
            engine = CreateEngine(culture);
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[warn] speech recognition unavailable: {ex.Message}");
            return (ListenOutcome.Unavailable, null);
        }

        using (engine)
        {
            var gate = new object();
            string? heard = null;
            engine.SpeechRecognized += (_, e) =>
            {
                if (!string.IsNullOrWhiteSpace(e.Result.Text))
                    lock (gate) { heard = e.Result.Text; }
            };
            engine.RecognizeAsync(RecognizeMode.Multiple);
            try
            {
                while (true)
                {
                    lock (gate)
                    {
                        if (heard is not null)
                            return (ListenOutcome.Recognized, heard);
                    }
                    if (toggleRequested())
                        return (ListenOutcome.Toggled, null);
                    Thread.Sleep(50);
                }
            }
            finally
            {
                try { engine.RecognizeAsyncCancel(); } catch { /* ignore */ }
            }
        }
    }
}
