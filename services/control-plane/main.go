package main

import (
	"fmt"
	"log"
	"net/http"
)

func main() {
	http.HandleFunc("/healthz", func(w http.ResponseWriter, _ *http.Request) {
		_, _ = fmt.Fprintln(w, "ok")
	})

	log.Println("fasttransfer control-plane listening on :8080")
	log.Fatal(http.ListenAndServe(":8080", nil))
}
