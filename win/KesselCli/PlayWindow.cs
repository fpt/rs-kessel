using System.Drawing;
using System.Drawing.Imaging;
using System.Drawing.Drawing2D;
using System.Runtime.InteropServices;
using System.Windows.Forms;
using uniffi.kessel_core;

namespace KesselCli;

// Windows analogue of the macOS play window (swift/Sources/KesselCli/PlayWindow.swift).
// `kessel --play <file.ux|file.asm>` loads a ROM into a standalone VmPlayer (no LLM
// needed), opens a WinForms window, and renders the 128x128 framebuffer scaled up
// with nearest-neighbour at 60 Hz, mapping the keyboard to the gamepad.
internal static class PlayWindow
{
    /// Load `romPath` and run the game window. Returns a process exit code.
    public static int Run(string romPath)
    {
        string source;
        try
        {
            source = File.ReadAllText(romPath);
        }
        catch (Exception e)
        {
            Console.Error.WriteLine($"Cannot read '{romPath}': {e.Message}");
            return 1;
        }

        var player = new VmPlayer();
        var error = player.Load(source, romPath);
        if (!string.IsNullOrEmpty(error))
        {
            Console.Error.WriteLine($"Failed to load '{romPath}':\n{error}");
            return 1;
        }

        // WinForms requires an STA thread; the console entry point is MTA, so run
        // the UI on a dedicated STA thread and wait for it.
        int exitCode = 0;
        var ui = new Thread(() =>
        {
            try
            {
                Application.EnableVisualStyles();
                Application.SetCompatibleTextRenderingDefault(false);
                Console.Error.WriteLine(
                    "[play] arrows move, Z/X = A/B, Enter/Space = Start/Select. Close the window to quit.");
                Application.Run(new PlayForm(player, Path.GetFileName(romPath)));
            }
            catch (Exception e)
            {
                Console.Error.WriteLine($"Play window error: {e.Message}");
                exitCode = 1;
            }
        });
        ui.SetApartmentState(ApartmentState.STA);
        ui.Start();
        ui.Join();
        return exitCode;
    }
}

/// The game window: a 60 Hz timer ticks the VM and blits the framebuffer.
internal sealed class PlayForm : Form
{
    private readonly VmPlayer _player;
    private readonly int _dim;
    private readonly System.Windows.Forms.Timer _timer;
    private readonly Bitmap _frame;
    private readonly byte[] _bgra;
    private byte _pressed;

    public PlayForm(VmPlayer player, string title)
    {
        _player = player;
        _dim = (int)player.ScreenDim();
        _frame = new Bitmap(_dim, _dim, PixelFormat.Format32bppArgb);
        _bgra = new byte[_dim * _dim * 4];

        const int scale = 5;
        Text = $"Kessel — {title}";
        ClientSize = new Size(_dim * scale, _dim * scale);
        FormBorderStyle = FormBorderStyle.FixedSingle;
        MaximizeBox = false;
        StartPosition = FormStartPosition.CenterScreen;
        DoubleBuffered = true;
        KeyPreview = true;

        _timer = new System.Windows.Forms.Timer { Interval = 16 }; // ~60 Hz
        _timer.Tick += (_, _) => Step();
        _timer.Start();
    }

    private void Step()
    {
        _player.Tick(_pressed);
        var rgba = _player.FramebufferRgba();
        if (rgba is { Length: > 0 })
        {
            WriteFramebuffer(rgba);
            Invalidate();
        }
    }

    // Our buffer is RGBA; GDI+ Format32bppArgb is BGRA in memory — swap R/B.
    private void WriteFramebuffer(byte[] rgba)
    {
        int n = _dim * _dim;
        for (int i = 0; i < n; i++)
        {
            int j = i * 4;
            _bgra[j + 0] = rgba[j + 2]; // B
            _bgra[j + 1] = rgba[j + 1]; // G
            _bgra[j + 2] = rgba[j + 0]; // R
            _bgra[j + 3] = rgba[j + 3]; // A
        }
        var rect = new Rectangle(0, 0, _dim, _dim);
        var data = _frame.LockBits(rect, ImageLockMode.WriteOnly, PixelFormat.Format32bppArgb);
        Marshal.Copy(_bgra, 0, data.Scan0, _bgra.Length);
        _frame.UnlockBits(data);
    }

    protected override void OnPaint(PaintEventArgs e)
    {
        e.Graphics.InterpolationMode = InterpolationMode.NearestNeighbor;
        e.Graphics.PixelOffsetMode = PixelOffsetMode.Half;
        e.Graphics.DrawImage(_frame, ClientRectangle);
    }

    private static byte? Bit(Keys k) => k switch
    {
        Keys.Left => (byte)0x01,
        Keys.Right => (byte)0x02,
        Keys.Up => (byte)0x04,
        Keys.Down => (byte)0x08,
        Keys.Z => (byte)0x10,
        Keys.X => (byte)0x20,
        Keys.Enter => (byte)0x40,
        Keys.Space => (byte)0x80,
        _ => null,
    };

    protected override void OnKeyDown(KeyEventArgs e)
    {
        var b = Bit(e.KeyCode);
        if (b.HasValue) { _pressed |= b.Value; e.Handled = true; }
        else base.OnKeyDown(e);
    }

    protected override void OnKeyUp(KeyEventArgs e)
    {
        var b = Bit(e.KeyCode);
        if (b.HasValue) { _pressed &= (byte)~b.Value; e.Handled = true; }
        else base.OnKeyUp(e);
    }

    protected override void OnFormClosed(FormClosedEventArgs e)
    {
        _timer.Stop();
        _timer.Dispose();
        _frame.Dispose();
        base.OnFormClosed(e);
    }
}
