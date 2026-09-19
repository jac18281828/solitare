import * as cdk from 'aws-cdk-lib';
import { Match, Template } from 'aws-cdk-lib/assertions';

import { SolitareStack } from './solitare-stack';

describe('SolitareStack', () => {
  const app = new cdk.App();
  const stack = new SolitareStack(app, 'TestSolitareStack', {
    env: {
      account: '504242000181',
      region: 'us-east-1',
    },
  });

  const template = Template.fromStack(stack);

  it('creates exactly one of each core resource, including an owned bucket', () => {
    template.resourceCountIs('AWS::CloudFront::Distribution', 1);
    template.resourceCountIs('AWS::CertificateManager::Certificate', 1);
    // Owned bucket, provisioned by this stack rather than imported — a
    // regression to an imported bucket would drop this resource entirely.
    template.resourceCountIs('AWS::S3::Bucket', 1);
    // A second policy resource on one bucket is legal at synth and fails at
    // deploy; S3 permits only one policy per bucket.
    template.resourceCountIs('AWS::S3::BucketPolicy', 1);
    template.resourceCountIs('AWS::Route53::RecordSet', 2);
  });

  it('names, encrypts and locks down the origin bucket', () => {
    template.hasResourceProperties('AWS::S3::Bucket', {
      BucketName: 'solitare-us-east-1-504242000181',
      BucketEncryption: {
        ServerSideEncryptionConfiguration: Match.arrayWith([
          Match.objectLike({
            ServerSideEncryptionByDefault: { SSEAlgorithm: 'AES256' },
          }),
        ]),
      },
      PublicAccessBlockConfiguration: {
        BlockPublicAcls: true,
        BlockPublicPolicy: true,
        IgnorePublicAcls: true,
        RestrictPublicBuckets: true,
      },
    });
  });

  it('destroys the bucket with the stack', () => {
    // DeletionPolicy sits beside Properties, so hasResourceProperties cannot
    // see it. A retained bucket blocks the next deploy with BucketAlreadyExists.
    template.hasResource('AWS::S3::Bucket', {
      DeletionPolicy: 'Delete',
      UpdateReplacePolicy: 'Delete',
    });
  });

  it('empties the bucket before teardown via the auto-delete custom resource', () => {
    const bucketLogicalIds = Object.keys(template.findResources('AWS::S3::Bucket'));
    expect(bucketLogicalIds).toHaveLength(1);
    const [bucketLogicalId] = bucketLogicalIds;

    const handlerRoleLogicalIds = Object.keys(template.findResources('AWS::IAM::Role'));
    expect(handlerRoleLogicalIds).toHaveLength(1);
    const [handlerRoleLogicalId] = handlerRoleLogicalIds;

    // BucketName must reference this stack's own bucket, not merely exist —
    // a custom resource pointed at a different bucket would pass a bare
    // resource-count check while leaving this bucket un-emptied.
    template.hasResourceProperties('Custom::S3AutoDeleteObjects', {
      BucketName: { Ref: bucketLogicalId },
    });

    // The handler role needs an explicit delete grant on this bucket, or
    // the custom resource exists but fails at runtime with AccessDenied.
    template.hasResourceProperties('AWS::S3::BucketPolicy', {
      Bucket: { Ref: bucketLogicalId },
      PolicyDocument: {
        Statement: Match.arrayWith([
          Match.objectLike({
            Effect: 'Allow',
            Action: Match.arrayWith(['s3:DeleteObject*']),
            Principal: {
              AWS: {
                'Fn::GetAtt': [handlerRoleLogicalId, 'Arn'],
              },
            },
          }),
        ]),
      },
    });
  });

  it('scopes the bucket policy to CloudFront by Sid, with the expected effects', () => {
    const bucketLogicalIds = Object.keys(template.findResources('AWS::S3::Bucket'));
    expect(bucketLogicalIds).toHaveLength(1);
    const [bucketLogicalId] = bucketLogicalIds;

    // Matched by statement rather than by bucket name: the policy references
    // its bucket by Ref, so a literal-name assertion cannot pass.
    template.hasResourceProperties('AWS::S3::BucketPolicy', {
      Bucket: { Ref: Match.anyValue() },
      PolicyDocument: {
        Statement: Match.arrayWith([
          Match.objectLike({
            Sid: 'AllowCloudFrontServicePrincipalReadOnly',
            Effect: 'Allow',
            Action: 's3:GetObject',
            Principal: { Service: 'cloudfront.amazonaws.com' },
          }),
          Match.objectLike({
            Sid: 'AllowCloudFrontServicePrincipalList',
            Effect: 'Allow',
            Action: 's3:ListBucket',
            Principal: { Service: 'cloudfront.amazonaws.com' },
            // The bucket itself, not `/*` — ListBucket is a bucket-level
            // action, distinct from the object-level GetObject grant above.
            Resource: { 'Fn::GetAtt': [bucketLogicalId, 'Arn'] },
            Condition: {
              StringEquals: {
                'AWS:SourceArn': Match.anyValue(),
                'AWS:SourceAccount': Match.anyValue(),
              },
            },
          }),
          Match.objectLike({
            Sid: 'DenyDirectS3ReadForObjects',
            Effect: 'Deny',
            Action: 's3:GetObject',
            Principal: { AWS: '*' },
          }),
        ]),
      },
    });
  });

  it('secures the origin with Origin Access Control, not legacy OAI', () => {
    const oacLogicalIds = Object.keys(template.findResources('AWS::CloudFront::OriginAccessControl'));
    expect(oacLogicalIds).toHaveLength(1);

    template.hasResourceProperties('AWS::CloudFront::Distribution', {
      DistributionConfig: Match.objectLike({
        Origins: Match.arrayWith([
          Match.objectLike({
            OriginAccessControlId: {
              'Fn::GetAtt': [oacLogicalIds[0], 'Id'],
            },
          }),
        ]),
      }),
    });

    template.resourceCountIs('AWS::CloudFront::CloudFrontOriginAccessIdentity', 0);
  });

  it('certifies solitare.2ad.com and serves it from the distribution', () => {
    template.hasResourceProperties('AWS::CertificateManager::Certificate', {
      DomainName: 'solitare.2ad.com',
    });

    template.hasResourceProperties('AWS::CloudFront::Distribution', {
      DistributionConfig: Match.objectLike({
        Aliases: Match.arrayWith(['solitare.2ad.com']),
      }),
    });
  });

  it('reports a true 404 for missing paths instead of the SPA fallback', () => {
    // Match.arrayWith cannot assert absence, so the synthesized
    // CustomErrorResponses array is inspected directly for the 403 entry
    // and the 200 response that this stack must no longer produce.
    const [distribution] = Object.values(template.findResources('AWS::CloudFront::Distribution'));
    const errorResponses: Array<Record<string, unknown>> =
      distribution.Properties.DistributionConfig.CustomErrorResponses;

    expect(distribution.Properties.DistributionConfig.DefaultRootObject).toBe('index.html');
    expect(errorResponses.some((response) => response.ErrorCode === 403)).toBe(false);
    expect(errorResponses.some((response) => response.ResponseCode === 200)).toBe(false);
    expect(errorResponses).toEqual([
      expect.objectContaining({
        ErrorCode: 404,
        ResponseCode: 404,
        ResponsePagePath: '/index.html',
      }),
    ]);
  });

  it('creates A and AAAA alias records in the imported zone', () => {
    template.hasResourceProperties('AWS::Route53::RecordSet', {
      Name: 'solitare.2ad.com.',
      Type: 'A',
      HostedZoneId: 'Z09862671HYH6ZFKNPGNL',
    });

    template.hasResourceProperties('AWS::Route53::RecordSet', {
      Name: 'solitare.2ad.com.',
      Type: 'AAAA',
      HostedZoneId: 'Z09862671HYH6ZFKNPGNL',
    });
  });

  it('refuses any region but us-east-1, where CloudFront requires its certificate', () => {
    const otherApp = new cdk.App();
    expect(
      () =>
        new SolitareStack(otherApp, 'WrongRegionSolitareStack', {
          env: {
            account: '504242000181',
            region: 'us-west-2',
          },
        }),
    ).toThrow(/us-east-1/);
  });
});
