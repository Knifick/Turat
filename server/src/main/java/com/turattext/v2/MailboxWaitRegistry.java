package com.turattext.v2;

import jakarta.annotation.PreDestroy;
import org.springframework.stereotype.Component;

import java.util.Map;
import java.util.Set;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.CopyOnWriteArraySet;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Ожидающие читатели почтовых ящиков. Клиент держит запрос открытым, Node будит его в тот
 * момент, когда конверт лёг в ящик, — поэтому сообщение приходит за доли секунды, а не к
 * следующему циклу опроса.
 *
 * <p>Реестр знает только идентификатор ящика: ни отправителя, ни содержимого конверта здесь нет.
 */
@Component
public class MailboxWaitRegistry {
    /** Верхняя граница на число ожидающих: реестр не должен расти от чужого трафика. */
    private static final int MAX_WAITERS = 20_000;

    private final Map<UUID, Set<Runnable>> waiters = new ConcurrentHashMap<>();
    private final AtomicInteger size = new AtomicInteger();
    private final ExecutorService wakeups = Executors.newFixedThreadPool(4, runnable -> {
        Thread thread = new Thread(runnable, "mailbox-wakeup");
        thread.setDaemon(true);
        return thread;
    });

    /**
     * Ставит читателя в очередь ожидания. Возвращает {@code false}, если реестр переполнен, —
     * тогда вызывающий отвечает сразу и клиент возвращается к обычному опросу.
     */
    public boolean await(UUID mailboxId, Runnable onEnvelope) {
        if (size.get() >= MAX_WAITERS) return false;
        waiters.computeIfAbsent(mailboxId, ignored -> new CopyOnWriteArraySet<>()).add(onEnvelope);
        size.incrementAndGet();
        return true;
    }

    public void cancel(UUID mailboxId, Runnable onEnvelope) {
        waiters.computeIfPresent(mailboxId, (ignored, current) -> {
            if (current.remove(onEnvelope)) size.decrementAndGet();
            return current.isEmpty() ? null : current;
        });
    }

    /** Будит всех, кто ждёт этот ящик. Пробуждение уходит в отдельный поток: отправитель не ждёт. */
    public void awaken(UUID mailboxId) {
        Set<Runnable> pending = waiters.remove(mailboxId);
        if (pending == null || pending.isEmpty()) return;
        size.addAndGet(-pending.size());
        for (Runnable waiter : pending) {
            try {
                wakeups.execute(waiter);
            } catch (RuntimeException ignored) {
                // Очередь пробуждений переполнена: клиент дождётся конца окна и заберёт конверт сам.
            }
        }
    }

    @PreDestroy
    void shutdown() {
        wakeups.shutdownNow();
    }
}
