FROM eclipse-temurin:21-jre
RUN useradd --system --uid 10001 --create-home turattext
WORKDIR /app
COPY turattext-server.jar /app/turattext-server.jar
RUN mkdir -p /app/data && chown -R turattext:turattext /app
USER 10001
EXPOSE 8080
EXPOSE 3479/udp
ENTRYPOINT ["java", "-XX:MaxRAMPercentage=75", "-jar", "/app/turattext-server.jar"]
