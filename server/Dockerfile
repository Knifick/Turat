FROM gradle:9.6.1-jdk21 AS build
WORKDIR /src
COPY build.gradle settings.gradle ./
COPY src ./src
RUN gradle --no-daemon clean bootJar

FROM eclipse-temurin:21-jre
RUN useradd --system --uid 10001 --create-home turattext
WORKDIR /app
COPY --from=build /src/build/libs/*.jar /app/turattext-server.jar
RUN mkdir -p /app/data && chown -R turattext:turattext /app
USER 10001
EXPOSE 8080
ENTRYPOINT ["java", "-XX:MaxRAMPercentage=75", "-jar", "/app/turattext-server.jar"]

