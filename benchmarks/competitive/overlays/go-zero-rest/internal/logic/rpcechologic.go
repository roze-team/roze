package logic

import (
	"context"

	"go-zero-rest/internal/svc"
	"go-zero-rest/internal/types"
	"go-zero-rpc/competitiveclient"

	"github.com/zeromicro/go-zero/core/logx"
)

type RpcEchoLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

func NewRpcEchoLogic(ctx context.Context, svcCtx *svc.ServiceContext) *RpcEchoLogic {
	return &RpcEchoLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *RpcEchoLogic) RpcEcho(req *types.EchoRequest) (*types.EchoResponse, error) {
	response, err := l.svcCtx.Competitive.Echo(
		l.ctx,
		&competitiveclient.EchoRequest{Payload: []byte(req.Payload)},
	)
	if err != nil {
		return nil, err
	}
	return &types.EchoResponse{Payload: string(response.Payload)}, nil
}
