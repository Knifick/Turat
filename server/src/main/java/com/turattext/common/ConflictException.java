package com.turattext.common;

/** Запрос корректен, но противоречит текущему состоянию: имя уже занято, версия устарела. */
public class ConflictException extends RuntimeException {
    public ConflictException(String message) {
        super(message);
    }
}
