Log Collector Lambda Extension
=============

This extension runs a local server to receive log entries from your lambda app, and batch write them into specified S3 bucket in parquet format. Logs in the log bucket can then be queried directly using Amazon Athena.

## When to use?

1. Mainly when you need to overcome 256kb event size hard-limit of Cloudwatch Logs, and don't want to run always-on server (Elasticsearch or Postgres) to store logs. Suitable for logging HTTP requests with full body for auditing purpose.

2. Want to use familiar SQL syntax to query/transform logs, using Amazon Athena.


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

When added as a layer to a function, the extension will start collecting log entries from JSON POST requests to **http://localhost:3030/collect**. See note below for payload format. Set environment variable `LCLE_S3_BUCKET` to tell the extension where to store logs. The lambda function must have permission to write to the specified bucket.

**Notes:**

1. Log entries are JSON objects with only one-level deep, and value can only be JSON primary value (bool, number, string). E.g `{"user_id":1}` is allowed, but `{"user_ids": [1, 2]}` is not.
2. Log entries are partitioned by "date" and "hour" (in UTC). You can fork this project to customize partitioning per your requirements.
3. To support automatic schema-detection, `null` is not allowed as value. Use a fallback value for corresponding type, e.g `0` for integers, or `""` (empty string) for strings.

**Configuration:**

|Environment Variable|Description|Default|
|---|---|---|
|LCLE_S3_BUCKET|Where to store logs|*None*|
|LCLE_S3_BUCKET_PREFIX|To which "folder" in the bucket to store logs|*write directly to bucket root*|
|LCLE_PORT|Which port to listen to|3030|
|LCLE_WRITE_LOCAL_DIR|For extension development: where to store logs locally|*None*|


**Setting-up Athena:**

After logs are first written to the bucket (when AWS fully stops the container that powers the Lambda function, around 10-20 minutes after last execution), you can use [AWS Glue Crawler to quickly read and set-up table definition from the bucket](https://docs.aws.amazon.com/athena/latest/ug/data-sources-glue.html).

Afterward, it is recommended to use [partition projection](https://docs.aws.amazon.com/athena/latest/ug/partition-projection.html) to not have to re-crawl for new partitions. Add these attributes to your table to enable partition projection:

|Key|Value|
|---|---|
|projection.enabled|true|
|projection.date.type|date|
|projection.date.format|yyyy-MM-dd|
|projection.date.range|2023-01-01,NOW|
|projection.hour.type|integer|
|projection.hour.range|0,23|
|projection.hour.digits|2|