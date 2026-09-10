using System.Windows.Input;

namespace TuratText.Client.ViewModels;

/// <summary>Same as <see cref="AsyncRelayCommand"/>, but forwards the bound CommandParameter
/// (e.g. the row's data item) to the handler instead of ignoring it.</summary>
public sealed class AsyncRelayCommand<T> : ICommand
{
    private readonly Func<T?, Task> _execute;
    private readonly Func<T?, bool>? _canExecute;
    private bool _isRunning;

    public AsyncRelayCommand(Func<T?, Task> execute, Func<T?, bool>? canExecute = null)
    {
        _execute = execute;
        _canExecute = canExecute;
    }

    public event EventHandler? CanExecuteChanged;

    public bool CanExecute(object? parameter) => !_isRunning && (_canExecute?.Invoke((T?)parameter) ?? true);

    public async void Execute(object? parameter)
    {
        if (!CanExecute(parameter)) {
            return;
        }

        try {
            _isRunning = true;
            RaiseCanExecuteChanged();
            await _execute((T?)parameter);
        } finally {
            _isRunning = false;
            RaiseCanExecuteChanged();
        }
    }

    public void RaiseCanExecuteChanged() => CanExecuteChanged?.Invoke(this, EventArgs.Empty);
}
