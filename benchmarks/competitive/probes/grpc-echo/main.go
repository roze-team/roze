package main

import (
	"bytes"
	"context"
	"flag"
	"fmt"
	"os"
	"time"

	"go-zero-rpc/competitive/competitive"

	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
)

func main() {
	endpoint := flag.String("endpoint", "127.0.0.1:19090", "gRPC endpoint")
	flag.Parse()

	conn, err := grpc.NewClient(
		*endpoint,
		grpc.WithTransportCredentials(insecure.NewCredentials()),
	)
	if err != nil {
		fatalf("connect: %v", err)
	}
	defer conn.Close()

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	payload := bytes.Repeat([]byte{'r'}, 1024)
	response, err := competitive.NewCompetitiveClient(conn).Echo(
		ctx,
		&competitive.EchoRequest{Payload: payload},
	)
	if err != nil {
		fatalf("echo: %v", err)
	}
	if !bytes.Equal(response.Payload, payload) {
		fatalf(
			"payload mismatch: sent=%d received=%d",
			len(payload),
			len(response.Payload),
		)
	}
	fmt.Printf("grpc echo valid: endpoint=%s payloadBytes=%d\n", *endpoint, len(payload))
}

func fatalf(format string, args ...any) {
	fmt.Fprintf(os.Stderr, format+"\n", args...)
	os.Exit(1)
}
