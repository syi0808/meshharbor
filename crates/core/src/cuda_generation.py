from pixalcuda import cli


class TunedModelStore(cli.ModelStore):
    def config(self):
        config = super().config()
        for stage in _meshharbor_sampler_overrides:
            config[stage]["params"] = _meshharbor_tune_sampler(stage, config[stage]["params"])
        return config


cli.ModelStore = TunedModelStore
cli.main()
