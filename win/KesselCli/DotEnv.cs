namespace KesselCli;

/// Minimal `.env` loader so API keys (e.g. OPENAI_API_KEY) can live in a local
/// file instead of the shell environment or the config.
///
/// Parses `KEY=VALUE` lines, tolerating `export KEY=...`, `#` comments, blank
/// lines, and surrounding single/double quotes. Variables already present in the
/// process environment are NOT overridden (real env wins). Files are searched in
/// priority order (most local first); each existing one is applied, so a global
/// file fills gaps left by a project-local one:
///   1. nearest `.env` walking up from the current directory
///   2. nearest `.env` walking up from the executable's directory (repo root in dev)
///   3. ~/.cache/kessel/.env  (alongside the user config)
internal static class DotEnv
{
    /// Load all discovered `.env` files; returns the paths that were applied.
    public static IReadOnlyList<string> Load()
    {
        var loaded = new List<string>();
        var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var path in Candidates())
        {
            if (path is null || !File.Exists(path)) continue;
            var full = Path.GetFullPath(path);
            if (!seen.Add(full)) continue; // de-dup (cwd and exe may share a .env)
            try
            {
                Apply(File.ReadAllLines(full));
                loaded.Add(full);
            }
            catch
            {
                /* unreadable .env — ignore */
            }
        }
        return loaded;
    }

    private static IEnumerable<string?> Candidates()
    {
        yield return FindUp(Directory.GetCurrentDirectory());
        yield return FindUp(AppContext.BaseDirectory);
        var home = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        yield return Path.Combine(home, ".cache", "kessel", ".env");
    }

    private static string? FindUp(string start)
    {
        for (var dir = new DirectoryInfo(start); dir is not null; dir = dir.Parent)
        {
            var candidate = Path.Combine(dir.FullName, ".env");
            if (File.Exists(candidate)) return candidate;
        }
        return null;
    }

    private static void Apply(string[] lines)
    {
        foreach (var raw in lines)
        {
            var line = raw.Trim();
            if (line.Length == 0 || line[0] == '#') continue;
            if (line.StartsWith("export ", StringComparison.Ordinal))
                line = line[7..].TrimStart();

            var eq = line.IndexOf('=');
            if (eq <= 0) continue;

            var key = line[..eq].Trim();
            if (key.Length == 0) continue;
            // Real environment variables take precedence over the .env file.
            if (!string.IsNullOrEmpty(Environment.GetEnvironmentVariable(key))) continue;

            Environment.SetEnvironmentVariable(key, StripQuotes(line[(eq + 1)..].Trim()));
        }
    }

    private static string StripQuotes(string v) =>
        v.Length >= 2 && ((v[0] == '"' && v[^1] == '"') || (v[0] == '\'' && v[^1] == '\''))
            ? v[1..^1]
            : v;
}
