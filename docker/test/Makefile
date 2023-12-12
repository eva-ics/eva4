all:
	@echo select target

run:
	docker compose up

clean:
	docker compose down
	sudo rm -rf ./data/runtime/registry ./data/runtime/xc ./data/runtime/.container_setup_completed
