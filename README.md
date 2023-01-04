Log Collector Lambda Extension
=============

This extension runs a local server to receive log entries from your lambda app, and batch write them into specified S3 bucket under parquet format. Logs in the log bucket can then be queried directly using Amazon Athena.

## How to build the extension

Run following command:

```
$ docker-compose run --rm dev ./package-extension.sh
```

Above command will output a zip file at **target/log_collector_lambda_extension.zip**, which can be uploaded (either with AWS CLI, or AWS Console) to create a Lambda layer.

**Side note:** To share a layer across accounts in [an organization](https://console.aws.amazon.com/organizations/v2/home), following command can be used:

```
$ aws lambda add-layer-version-permission --layer-name <layer-name> --version-number <layer-version> --statement-id allaccount --action lambda:GetLayerVersion --principal '*' --organization-id <org-id-in-o-xxxxxxxx-format>
```

## How to use and configure the extension

When added as a layer to a function, the extension will start collecting log entries from JSON POST requests to **http://localhost:3030/collect**. Configure `LCLE_S3_BUCKET` to tell the extension where to store logs. The lambda function must have permission to write to the specified bucket.

After logs are first written to the bucket (when AWS fully stops the container that run the Lambda function), you can start [setting-up Amazon Athena to read from the bucket](https://docs.aws.amazon.com/athena/latest/ug/data-sources-glue.html). 

**Notes:**

1. Log entries are JSON objects with only one-level deep, and value can only be JSON primary value (bool, number, string). E.g `{"user_id":1}` is allowed, but `{"user_ids": [1, 2]}` is not.
2. To support automatic schema-detection, `null` is not allowed as value. Use a fallback value for corresponding type, e.g `0` for integers, or `""` (empty string) for strings.

**Configuration:**

|Environment Variable|Description|Default|
|---|---|---|
|LCLE_S3_BUCKET|Where to store logs|*None*|
|LCLE_S3_BUCKET_PREFIX|To which "folder" in the bucket to store logs|*write directly to bucket root*|
|LCLE_PORT|Which port to listen to|3030|
|LCLE_WRITE_LOCAL_DIR|For extension development: where to store logs locally|*None*|