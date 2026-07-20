package logic

import (
	"context"

	"go-zero-rest/internal/svc"
	"go-zero-rest/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type EchoLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

func NewEchoLogic(ctx context.Context, svcCtx *svc.ServiceContext) *EchoLogic {
	return &EchoLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *EchoLogic) Echo(req *types.EchoRequest) (*types.EchoResponse, error) {
	return &types.EchoResponse{Payload: req.Payload}, nil
}
