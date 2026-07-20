package logic

import (
	"context"

	"go-zero-rpc/competitive/competitive"
	"go-zero-rpc/internal/svc"

	"github.com/zeromicro/go-zero/core/logx"
)

type EchoLogic struct {
	ctx    context.Context
	svcCtx *svc.ServiceContext
	logx.Logger
}

func NewEchoLogic(ctx context.Context, svcCtx *svc.ServiceContext) *EchoLogic {
	return &EchoLogic{
		ctx:    ctx,
		svcCtx: svcCtx,
		Logger: logx.WithContext(ctx),
	}
}

func (l *EchoLogic) Echo(in *competitive.EchoRequest) (*competitive.EchoResponse, error) {
	return &competitive.EchoResponse{Payload: in.Payload}, nil
}
