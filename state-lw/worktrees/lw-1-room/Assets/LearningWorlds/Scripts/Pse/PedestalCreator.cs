using UnityEngine;

public static class PedestalCreator
{
    public static GameObject CreateLearningStation(Vector3 position, string label)
    {
        GameObject pedestalGroup = new GameObject(label + "Pedestal");
        pedestalGroup.transform.position = position;

        // Post structure
        GameObject postRoot = new GameObject("Post");
        postRoot.transform.SetParent(pedestalGroup.transform);
        CreatePost(postRoot.transform);

        // Learning station network name
        GameObject labelTextObject = new GameObject("Label");
        labelTextObject.transform.SetParent(pedestalGroup.transform);
        CreateText(labelTextObject.transform, label);

        return pedestalGroup;
    }

    public static GameObject CreateQuizAnswerPedestal()
    {
        GameObject pedestalGroup = new GameObject("QuizAnswerPedestal");
        pedestalGroup.transform.position = Vector3.zero;

        // Cube-shaped pedestal
        GameObject answerPedestal = GameObject.CreatePrimitive(PrimitiveType.Cube);
        answerPedestal.transform.SetParent(pedestalGroup.transform);
        answerPedestal.transform.localScale = Vector3.one * 1.5f;
        answerPedestal.transform.position = Vector3.zero;
        answerPedestal.GetComponent<Renderer>().material = new Material(Shader.Find("Standard"))
        {
            color = Color.white
        };
        answerPedestal.name = "AnswerButton";

        // Add collider for display interactions
        BoxCollider bc = answerPedestal.AddComponent<BoxCollider>();
        bc.isTrigger = true;
        answerPedestal.AddComponent<QuizStation>().Initialize(SelectedElement.Instance);

        return pedestalGroup;
    }

    private static void CreatePost(Transform parent)
    {
        GameObject post = GameObject.CreatePrimitive(PrimitiveType.Cylinder);
        post.transform.SetParent(parent);
        post.transform.localScale = new Vector3(0.3f, 2f, 0.3f);
        post.transform.localPosition = new Vector3(0, -1, 0);
        post.GetComponent<Renderer>().material = new Material(Shader.Find("Standard"))
        {
            color = Color.grey
        };
        post.name = "PedestalPost";
    }

    private static void CreateText(Transform parent, string text)
    {
        GameObject label = new GameObject("Label");
        label.transform.SetParent(parent);
        TextMesh tm = label.AddComponent<TextMesh>();
        tm.text = text;
        tm.fontSize = 18;
        tm-alignment = TextAnchor.MiddleCenter;
        tm.transform.localPosition = new Vector3(0, 0.8f, 0.1f);
        tm.transform.localRotation = Quaternion.Euler(90, 0, 0);
        label.name = "StationLabel";
    }
}