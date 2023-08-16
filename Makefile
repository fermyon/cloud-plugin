OS_TYPE ?= 
ARCH ?=
ifeq ($(OS),Windows_NT)
	OS_TYPE = windows
	ARCH = amd64
else
	UNAME_S := $(shell uname -s)
	ifeq ($(UNAME_S),Linux)
		OS_TYPE = linux
	endif
	ifeq ($(UNAME_S),Darwin)
		OS_TYPE = macos
	endif
	UNAME_P := $(shell uname -p)
	ifeq ($(UNAME_P),x86_64)
		ARCH = amd64
	endif
	ifneq ($(filter arm%,$(UNAME_P)),)
		ARCH = aarch64
	endif
endif

.PHONY: build
build:
	cargo build --workspace --release --all-targets --all-features

.PHONY: test
test:
	cargo test --all --no-fail-fast -- --nocapture

.PHONY: lint
lint:
	cargo clippy --all-targets --all-features -- -D warnings
	cargo fmt --all -- --check

.PHONY: package
package:
	@cp target/release/cloud-plugin cloud
	@tar -czvf cloud.tar.gz cloud &>/dev/null
	@sha256sum cloud.tar.gz | cut -c -64 | tr -d '\n' > /tmp/cloud.sha256sum
	@rm cloud

.PHONY: install
install: package
	@curl -sLR -o /tmp/cloud.json https://github.com/fermyon/cloud-plugin/releases/download/canary/cloud.json
	@jq -j \
		--arg os $(OS_TYPE) \
		--arg arch $(ARCH) \
		--arg sha256sum "$$(</tmp/cloud.sha256sum)" \
		--arg url "file://$$(pwd)/cloud.tar.gz" \
		'(.packages[] | select(.os==$$os and .arch==$$arch) ).sha256 = $$sha256sum | (.packages[] | select(.os==$$os and .arch==$$arch) ).url = $$url' \
		/tmp/cloud.json > cloud.json
	spin plugin install -y -f ./cloud.json