package svc

import (
	"go-zero-rest/internal/config"
	"go-zero-rpc/competitiveclient"

	"github.com/zeromicro/go-zero/zrpc"
)

type ServiceContext struct {
	Config      config.Config
	Competitive competitiveclient.Competitive
}

func NewServiceContext(c config.Config) *ServiceContext {
	return &ServiceContext{
		Config:      c,
		Competitive: competitiveclient.NewCompetitive(zrpc.MustNewClient(c.CompetitiveRpc)),
	}
}
