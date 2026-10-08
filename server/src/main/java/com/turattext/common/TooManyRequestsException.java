package com.turattext.common;

/** Слишком много неудачных попыток подряд: вход временно закрыт. */
public class TooManyRequestsException extends RuntimeException {
    public TooManyRequestsException(String message) {
        super(message);
    }
}
