# The uniray-gateway image: two static binaries and the CA bundle, nothing
# else. Assembled by scripts/build-gateway.sh, which prepares the root
# filesystem and passes it as the "rootfs" build context.
FROM scratch
COPY --from=rootfs / /

# Xray is a Go program: keep its heap within a 64 MB container. The limit is
# soft; GC runs harder as it is approached instead of failing.
ENV GOMEMLIMIT=40MiB \
    GOGC=50 \
    PATH=/usr/bin

EXPOSE 9090 1080
ENTRYPOINT ["/usr/bin/uniray-gateway"]
