FROM alpine:latest
RUN apk add --no-cache dumb-init

COPY target/release/lagerist /app/
WORKDIR /app

VOLUME ["/proc", "/sys", "/dev"]
EXPOSE 9165
ENTRYPOINT ["dumb-init", "--"]
CMD [ "/app/lagerist" ]

# run using: docker run --privileged --rm -it -v /proc:/proc -v /dev:/dev:ro -v /sys:/sys lagerist
