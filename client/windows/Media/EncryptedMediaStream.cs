using TuratText.Windows.Interop;

namespace TuratText.Windows.Media;

/// <summary>
/// Поток поверх зашифрованного вложения.
/// </summary>
/// <remarks>
/// Вложение лежит на диске зашифрованным по чанкам, поэтому расшифровывать его целиком, чтобы
/// показать кадр или включить видео, не нужно: этот поток отдаёт ровно тот отрезок, который
/// запросил проигрыватель или декодер картинки. Благодаря этому проигрыватель
/// начинает воспроизведение сразу, перемотка в середину часового ролика не читает всё до неё, и
/// расшифрованная копия файла нигде не появляется.
/// </remarks>
internal sealed class EncryptedMediaStream : Stream
{
    private nint _handle;
    private long _position;

    private EncryptedMediaStream(nint handle, long length)
    {
        _handle = handle;
        Length = length;
    }

    /// <summary>Открывает вложение или возвращает <c>null</c>, если ключ или файл не подошли.</summary>
    public static EncryptedMediaStream? TryOpen(RustCore core, string path)
    {
        nint handle = core.OpenMedia(path);
        if (handle == 0) return null;
        long length = RustCore.MediaLength(handle);
        if (length < 0)
        {
            RustCore.CloseMedia(handle);
            return null;
        }
        return new EncryptedMediaStream(handle, length);
    }

    public override bool CanRead => _handle != 0;
    public override bool CanSeek => true;
    public override bool CanWrite => false;
    public override long Length { get; }

    public override long Position
    {
        get => _position;
        set => _position = Math.Clamp(value, 0, Length);
    }

    public override int Read(byte[] buffer, int offset, int count)
    {
        ArgumentNullException.ThrowIfNull(buffer);
        if (count <= 0 || _handle == 0) return 0;
        int wanted = (int)Math.Min(count, Length - _position);
        if (wanted <= 0) return 0;
        int read = RustCore.ReadMedia(_handle, _position, buffer, offset, wanted);
        if (read < 0) throw new IOException("Не удалось прочитать вложение");
        _position += read;
        return read;
    }

    public override long Seek(long offset, SeekOrigin origin)
    {
        long target = origin switch
        {
            SeekOrigin.Begin => offset,
            SeekOrigin.Current => _position + offset,
            SeekOrigin.End => Length + offset,
            _ => offset,
        };
        Position = target;
        return _position;
    }

    public override void Flush()
    {
    }

    public override void SetLength(long value) => throw new NotSupportedException();

    public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();

    protected override void Dispose(bool disposing)
    {
        nint handle = _handle;
        _handle = 0;
        if (handle != 0) RustCore.CloseMedia(handle);
        base.Dispose(disposing);
    }
}
